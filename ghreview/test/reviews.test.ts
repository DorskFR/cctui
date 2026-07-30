import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { upsertDocument } from "../src/db/documents.ts";
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

interface OctoOpts {
  headSha?: string;
  files?: string[];
  reviewCalls?: Record<string, unknown>[];
  existing?: Record<string, unknown>[];
}

function reviewOctokit(opts: OctoOpts = {}): OctokitRequest {
  const headSha = opts.headSha ?? "sha1";
  const files = opts.files ?? ["src/app.ts"];
  return {
    request: async (route: string, params?: Record<string, unknown>): Promise<OctokitResponse> => {
      if (route.startsWith("POST") && route.includes("/reviews")) {
        opts.reviewCalls?.push(params ?? {});
        return { status: 200, headers: {}, data: { id: 999 } };
      }
      if (route.includes("/files")) {
        const page = Number(params?.page ?? 1);
        const data = page === 1 ? files.map((filename) => ({ filename })) : [];
        return { status: 200, headers: {}, data };
      }
      if (route.endsWith("/comments")) {
        const page = Number(params?.page ?? 1);
        return { status: 200, headers: {}, data: page === 1 ? (opts.existing ?? []) : [] };
      }
      return {
        status: 200,
        headers: {},
        data: { number: 42, state: "open", head: { sha: headSha } },
      };
    },
  };
}

function deps(octokit: OctokitRequest, extra: Partial<AppDeps> = {}): AppDeps {
  const account = createAccount({ login: "alpha", token: undefined, octokit });
  return {
    db,
    auth,
    accountFor: (login) => (login === "alpha" ? account : undefined),
    ...extra,
  };
}

async function seedPull(headSha = "sha1"): Promise<void> {
  await upsertDocument(db, {
    account: "alpha",
    kind: "pull_request",
    key: "alpha/repo#42",
    etag: null,
    payload: { number: 42, state: "open", head: { sha: headSha } },
  });
}

const BASE = "/v1/repos/alpha/repo/pulls/42/review-draft";

guarded("review drafts + publish", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM review_drafts");
    await db.sql.unsafe("DELETE FROM documents");
    await db.sql.unsafe("DELETE FROM gh_accounts");
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: "x" });
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: "y" });
    await seedPull();
  });

  test("adding a comment opens a draft, captures head_sha, and is owner-scoped", async () => {
    const app = createApp(deps(reviewOctokit()));
    const res = await app.request(`${BASE}/comments`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", path: "src/app.ts", line: 10, body: "nit" }),
    });
    expect(res.status).toBe(201);
    const { draft } = (await res.json()) as { draft: { head_sha: string; comments: unknown[] } };
    expect(draft.head_sha).toBe("sha1");
    expect(draft.comments.length).toBe(1);

    const other = await app.request(`${BASE}?account=alpha`, { headers: B });
    expect(((await other.json()) as { draft: unknown }).draft).toBeNull();
  });

  test("edit and delete draft comments", async () => {
    const app = createApp(deps(reviewOctokit()));
    const add = await app.request(`${BASE}/comments`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", path: "src/app.ts", line: 10, body: "first" }),
    });
    const { draft } = (await add.json()) as { draft: { comments: { id: string }[] } };
    const id = draft.comments[0]?.id;

    const edited = await app.request(`${BASE}/comments/${id}`, {
      method: "PATCH",
      headers: A,
      body: JSON.stringify({ account: "alpha", body: "second" }),
    });
    const e = (await edited.json()) as { draft: { comments: { body: string }[] } };
    expect(e.draft.comments[0]?.body).toBe("second");

    const del = await app.request(`${BASE}/comments/${id}?account=alpha`, {
      method: "DELETE",
      headers: A,
    });
    const d = (await del.json()) as { draft: { comments: unknown[] } };
    expect(d.draft.comments.length).toBe(0);
  });

  test("publish posts one batched review and clears the draft", async () => {
    const reviewCalls: Record<string, unknown>[] = [];
    const app = createApp(deps(reviewOctokit({ reviewCalls, files: ["src/app.ts"] })));
    await app.request(`${BASE}/comments`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", path: "src/app.ts", line: 10, body: "nit" }),
    });

    const pub = await app.request(`${BASE}/publish`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", verdict: "approve", body: "LGTM" }),
    });
    expect(pub.status).toBe(200);
    const result = (await pub.json()) as { published: boolean; posted: number; skipped: unknown[] };
    expect(result.published).toBe(true);
    expect(result.posted).toBe(1);
    expect(result.skipped.length).toBe(0);

    expect(reviewCalls.length).toBe(1);
    expect(reviewCalls[0]?.event).toBe("APPROVE");
    expect((reviewCalls[0] as { comments: unknown[] }).comments.length).toBe(1);

    const after = await app.request(`${BASE}?account=alpha`, { headers: A });
    expect(((await after.json()) as { draft: unknown }).draft).toBeNull();
  });

  test("stale head-SHA guard rejects publish when the PR head moved", async () => {
    const reviewCalls: Record<string, unknown>[] = [];
    const app = createApp(deps(reviewOctokit({ reviewCalls, headSha: "sha2" })));
    await app.request(`${BASE}/comments`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({
        account: "alpha",
        path: "src/app.ts",
        line: 10,
        body: "nit",
        head_sha: "sha1",
      }),
    });

    const pub = await app.request(`${BASE}/publish`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", verdict: "comment", body: "" }),
    });
    expect(pub.status).toBe(409);
    const err = (await pub.json()) as {
      error: { code: string; details: { current_head: string } };
    };
    expect(err.error.code).toBe("stale_head");
    expect(err.error.details.current_head).toBe("sha2");
    expect(reviewCalls.length).toBe(0);
  });

  test("publish reports comments skipped because their path is not in the diff", async () => {
    const reviewCalls: Record<string, unknown>[] = [];
    const app = createApp(deps(reviewOctokit({ reviewCalls, files: ["src/app.ts"] })));
    await app.request(`${BASE}/comments`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", path: "src/app.ts", line: 10, body: "kept" }),
    });
    await app.request(`${BASE}/comments`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", path: "gone.ts", line: 3, body: "dropped" }),
    });

    const pub = await app.request(`${BASE}/publish`, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", verdict: "comment", body: "" }),
    });
    expect(pub.status).toBe(200);
    const result = (await pub.json()) as { posted: number; skipped: { path: string }[] };
    expect(result.posted).toBe(1);
    expect(result.skipped.length).toBe(1);
    expect(result.skipped[0]?.path).toBe("gone.ts");
    expect((reviewCalls[0] as { comments: unknown[] }).comments.length).toBe(1);
  });

  test("lists existing published review comments from GitHub", async () => {
    const existing = [
      {
        id: 1,
        path: "src/app.ts",
        line: 5,
        side: "RIGHT",
        body: "old thread",
        user: { login: "bob" },
      },
    ];
    const app = createApp(deps(reviewOctokit({ existing })));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/comments?account=alpha", {
      headers: A,
    });
    expect(res.status).toBe(200);
    const { items } = (await res.json()) as { items: { path: string; user: string }[] };
    expect(items.length).toBe(1);
    expect(items[0]?.path).toBe("src/app.ts");
    expect(items[0]?.user).toBe("bob");
  });

  test("rejects listing review threads under an account the caller does not own", async () => {
    const app = createApp(deps(reviewOctokit()));
    const res = await app.request("/v1/repos/alpha/repo/pulls/42/comments?account=alpha", {
      headers: B,
    });
    expect(res.status).toBe(403);
  });

  test("cannot publish a draft under an account the caller does not own", async () => {
    const app = createApp(deps(reviewOctokit()));
    const res = await app.request(`${BASE}/comments`, {
      method: "POST",
      headers: B,
      body: JSON.stringify({ account: "alpha", path: "src/app.ts", line: 10, body: "x" }),
    });
    expect(res.status).toBe(404);
  });
});
