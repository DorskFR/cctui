import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { getDocument, upsertDocument } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import { upsertSubscription } from "../src/db/subscriptions.ts";
import type { AppDeps } from "../src/deps.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA,tokB:userB"));
const A = { authorization: "Bearer tokA", "content-type": "application/json" };
const B = { authorization: "Bearer tokB", "content-type": "application/json" };

interface OctoOpts {
  headSha?: string;
  mergeStatus?: number;
  mergeMessage?: string;
  mergeCalls?: Record<string, unknown>[];
}

function mergeOctokit(opts: OctoOpts = {}): OctokitRequest {
  const headSha = opts.headSha ?? "sha1";
  return {
    request: async (route: string, params?: Record<string, unknown>): Promise<OctokitResponse> => {
      if (route.startsWith("PUT") && route.endsWith("/merge")) {
        opts.mergeCalls?.push(params ?? {});
        if (opts.mergeStatus && opts.mergeStatus >= 400) {
          throw {
            status: opts.mergeStatus,
            message: opts.mergeMessage ?? "rejected",
            response: { data: { message: opts.mergeMessage ?? "rejected" } },
          };
        }
        return { status: 200, headers: {}, data: { merged: true, sha: "merged1", message: "ok" } };
      }
      return {
        status: 200,
        headers: {},
        data: { number: 42, state: "open", head: { sha: headSha } },
      };
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
    payload: { number: 42, state: "open", head: { sha: "sha1" } },
  });
  await upsertSubscription(db, "alpha", "pull_request", "alpha/repo#42", "user");
}

const URL = "/v1/repos/alpha/repo/pulls/42/merge";

guarded("merge pull request", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string, "ghreview");
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM documents");
    await db.sql.unsafe("DELETE FROM subscriptions");
    await db.sql.unsafe("DELETE FROM gh_accounts");
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: "x" });
    await seedPull();
  });

  test("merges with the given method and removes the stored document", async () => {
    const mergeCalls: Record<string, unknown>[] = [];
    const app = createApp(deps(mergeOctokit({ mergeCalls })));
    const res = await app.request(URL, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", merge_method: "squash" }),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { merged: boolean; sha: string };
    expect(body.merged).toBe(true);
    expect(body.sha).toBe("merged1");
    expect(mergeCalls[0]?.merge_method).toBe("squash");

    const doc = await getDocument(db, "alpha", "pull_request", "alpha/repo#42");
    expect(doc).toBeNull();
    const subs = await db.sql`SELECT active FROM subscriptions WHERE target = 'alpha/repo#42'`;
    expect((subs[0] as { active: boolean })?.active).toBe(false);
  });

  test("maps a 405 not-mergeable rejection", async () => {
    const app = createApp(deps(mergeOctokit({ mergeStatus: 405, mergeMessage: "not mergeable" })));
    const res = await app.request(URL, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", merge_method: "merge" }),
    });
    expect(res.status).toBe(405);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("not_mergeable");
    const doc = await getDocument(db, "alpha", "pull_request", "alpha/repo#42");
    expect(doc).not.toBeNull();
  });

  test("rejects with 409 when the expected head SHA is stale", async () => {
    const mergeCalls: Record<string, unknown>[] = [];
    const app = createApp(deps(mergeOctokit({ headSha: "sha2", mergeCalls })));
    const res = await app.request(URL, {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "alpha", expected_head_sha: "sha1" }),
    });
    expect(res.status).toBe(409);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("stale_head");
    expect(mergeCalls.length).toBe(0);
  });

  test("cannot merge under an account the caller does not own", async () => {
    const app = createApp(deps(mergeOctokit()));
    const res = await app.request(URL, {
      method: "POST",
      headers: B,
      body: JSON.stringify({ account: "alpha" }),
    });
    expect(res.status).toBe(403);
  });
});
