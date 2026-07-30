import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { getDocument, upsertDocument } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import type { AppDeps } from "../src/deps.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA,tokB:userB"));

const A = { authorization: "Bearer tokA", "content-type": "application/json" };
const B = { authorization: "Bearer tokB", "content-type": "application/json" };

interface Reaction {
  id: number;
  content: string;
  user: { login: string };
}

interface OctoState {
  reactions: Reaction[];
  posts: Record<string, unknown>[];
  deletes: string[];
  nextId: number;
}

function reactionsOctokit(state: OctoState, login = "alpha"): OctokitRequest {
  return {
    request: async (route: string, params?: Record<string, unknown>): Promise<OctokitResponse> => {
      if (route.startsWith("GET") && route.endsWith("/reactions")) {
        const page = Number(params?.page ?? 1);
        return { status: 200, headers: {}, data: page === 1 ? state.reactions : [] };
      }
      if (route.startsWith("POST") && route.endsWith("/reactions")) {
        state.posts.push(params ?? {});
        state.reactions.push({
          id: state.nextId++,
          content: String(params?.content),
          user: { login },
        });
        return { status: 201, headers: {}, data: state.reactions.at(-1) };
      }
      if (route.startsWith("DELETE") && route.includes("/reactions/")) {
        const id = Number(route.split("/reactions/")[1]);
        state.deletes.push(String(id));
        state.reactions = state.reactions.filter((r) => r.id !== id);
        return { status: 204, headers: {}, data: null };
      }
      return { status: 200, headers: {}, data: {} };
    },
  };
}

function deps(octokit: OctokitRequest): AppDeps {
  const account = createAccount({ login: "alpha", token: undefined, octokit });
  return { db, auth, accountFor: (login) => (login === "alpha" ? account : undefined) };
}

async function seedPull(): Promise<void> {
  await upsertDocument(db, {
    account: "alpha",
    kind: "pull_request",
    key: "alpha/repo#42",
    etag: null,
    payload: { number: 42, state: "open", reactions: { "+1": 0, total_count: 0 } },
  });
}

guarded("reactions toggle", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM documents");
    await db.sql.unsafe("DELETE FROM gh_accounts");
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: "x" });
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: "y" });
    await seedPull();
  });

  test("first toggle creates the reaction and patches the PR document", async () => {
    const state: OctoState = { reactions: [], posts: [], deletes: [], nextId: 1 };
    const app = createApp(deps(reactionsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/reactions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", content: "+1" }),
    });
    expect(res.status).toBe(200);
    const summary = (await res.json()) as {
      "+1": number;
      total_count: number;
      viewer_reactions: string[];
    };
    expect(summary["+1"]).toBe(1);
    expect(summary.total_count).toBe(1);
    expect(summary.viewer_reactions).toEqual(["+1"]);
    expect(state.posts.length).toBe(1);
    expect(state.deletes.length).toBe(0);

    const doc = await getDocument(db, "alpha", "pull_request", "alpha/repo#42");
    const payload = doc?.payload as { reactions: { "+1": number; total_count: number } };
    const reactions = payload.reactions;
    expect(reactions["+1"]).toBe(1);
    expect(reactions.total_count).toBe(1);
  });

  test("second toggle of the same content removes the viewer's reaction", async () => {
    const state: OctoState = {
      reactions: [{ id: 7, content: "+1", user: { login: "alpha" } }],
      posts: [],
      deletes: [],
      nextId: 10,
    };
    const app = createApp(deps(reactionsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/reactions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", content: "+1" }),
    });
    expect(res.status).toBe(200);
    const summary = (await res.json()) as { "+1": number; viewer_reactions: string[] };
    expect(summary["+1"]).toBe(0);
    expect(summary.viewer_reactions).toEqual([]);
    expect(state.deletes).toEqual(["7"]);
    expect(state.posts.length).toBe(0);
  });

  test("only the viewer's own reaction is deleted, others are kept", async () => {
    const state: OctoState = {
      reactions: [
        { id: 1, content: "heart", user: { login: "bob" } },
        { id: 2, content: "heart", user: { login: "alpha" } },
      ],
      posts: [],
      deletes: [],
      nextId: 10,
    };
    const app = createApp(deps(reactionsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/issues/comments/5/reactions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", content: "heart" }),
    });
    expect(res.status).toBe(200);
    const summary = (await res.json()) as { heart: number; viewer_reactions: string[] };
    expect(summary.heart).toBe(1);
    expect(summary.viewer_reactions).toEqual([]);
    expect(state.deletes).toEqual(["2"]);
  });

  test("review-comment reactions route toggles via the pulls/comments endpoint", async () => {
    const state: OctoState = { reactions: [], posts: [], deletes: [], nextId: 1 };
    const app = createApp(deps(reactionsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/pulls/comments/9/reactions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", content: "rocket" }),
    });
    expect(res.status).toBe(200);
    const summary = (await res.json()) as { rocket: number };
    expect(summary.rocket).toBe(1);
    expect(state.posts.length).toBe(1);
  });

  test("rejects a toggle under an account the caller does not own", async () => {
    const state: OctoState = { reactions: [], posts: [], deletes: [], nextId: 1 };
    const app = createApp(deps(reactionsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/reactions", {
      method: "POST",
      headers: B,
      body: JSON.stringify({ account: "alpha", content: "+1" }),
    });
    expect(res.status).toBe(403);
  });
});
