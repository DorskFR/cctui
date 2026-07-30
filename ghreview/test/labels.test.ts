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
import { dbGate } from "./dbGate.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = dbGate(describe, DATABASE_URL);

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA,tokB:userB"));

const A = { authorization: "Bearer tokA", "content-type": "application/json" };
const B = { authorization: "Bearer tokB", "content-type": "application/json" };

interface Label {
  name: string;
  color: string;
  description: string | null;
}

interface OctoState {
  repoLabels: Label[];
  prLabels: Label[];
  posts: Record<string, unknown>[];
  deletes: string[];
}

function labelsOctokit(state: OctoState): OctokitRequest {
  return {
    request: async (route: string, params?: Record<string, unknown>): Promise<OctokitResponse> => {
      if (route.startsWith("GET") && route.endsWith("/labels")) {
        const page = Number(params?.page ?? 1);
        return { status: 200, headers: {}, data: page === 1 ? state.repoLabels : [] };
      }
      if (route.startsWith("POST") && route.endsWith("/labels")) {
        state.posts.push(params ?? {});
        const names = (params?.labels as string[]) ?? [];
        for (const name of names) {
          if (!state.prLabels.some((l) => l.name === name)) {
            const found = state.repoLabels.find((l) => l.name === name);
            state.prLabels.push(found ?? { name, color: "", description: null });
          }
        }
        return { status: 200, headers: {}, data: state.prLabels };
      }
      if (route.startsWith("DELETE") && route.includes("/labels/")) {
        const name = decodeURIComponent(route.split("/labels/")[1] ?? "");
        state.deletes.push(name);
        state.prLabels = state.prLabels.filter((l) => l.name !== name);
        return { status: 200, headers: {}, data: state.prLabels };
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
    payload: { number: 42, state: "open", labels: [] },
  });
}

guarded("labels", () => {
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

  test("lists a repository's labels for the picker", async () => {
    const state: OctoState = {
      repoLabels: [
        { name: "bug", color: "d73a4a", description: "Something isn't working" },
        { name: "docs", color: "0075ca", description: null },
      ],
      prLabels: [],
      posts: [],
      deletes: [],
    };
    const app = createApp(deps(labelsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/labels?account=alpha", { headers: A });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { items: Label[] };
    expect(body.items.map((l) => l.name)).toEqual(["bug", "docs"]);
    expect(body.items[0]?.color).toBe("d73a4a");
  });

  test("adds a label and patches the PR document", async () => {
    const state: OctoState = {
      repoLabels: [{ name: "bug", color: "d73a4a", description: "x" }],
      prLabels: [],
      posts: [],
      deletes: [],
    };
    const app = createApp(deps(labelsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/labels", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", name: "bug" }),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { labels: Label[] };
    expect(body.labels.map((l) => l.name)).toEqual(["bug"]);
    expect(state.posts).toEqual([{ labels: ["bug"] }]);

    const doc = await getDocument(db, "alpha", "pull_request", "alpha/repo#42");
    const payload = doc?.payload as { labels: Label[] };
    expect(payload.labels.map((l) => l.name)).toEqual(["bug"]);
    expect(payload.labels[0]?.color).toBe("d73a4a");
  });

  test("removes a label and patches the PR document", async () => {
    const state: OctoState = {
      repoLabels: [],
      prLabels: [
        { name: "bug", color: "d73a4a", description: null },
        { name: "docs", color: "0075ca", description: null },
      ],
      posts: [],
      deletes: [],
    };
    const app = createApp(deps(labelsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/labels/bug?account=alpha", {
      method: "DELETE",
      headers: A,
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { labels: Label[] };
    expect(body.labels.map((l) => l.name)).toEqual(["docs"]);
    expect(state.deletes).toEqual(["bug"]);

    const doc = await getDocument(db, "alpha", "pull_request", "alpha/repo#42");
    const payload = doc?.payload as { labels: Label[] };
    expect(payload.labels.map((l) => l.name)).toEqual(["docs"]);
  });

  test("rejects a label mutation under an account the caller does not own", async () => {
    const state: OctoState = { repoLabels: [], prLabels: [], posts: [], deletes: [] };
    const app = createApp(deps(labelsOctokit(state)));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/labels", {
      method: "POST",
      headers: B,
      body: JSON.stringify({ account: "alpha", name: "bug" }),
    });
    expect(res.status).toBe(403);
  });
});
