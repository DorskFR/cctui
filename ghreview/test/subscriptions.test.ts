import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { runMigrations } from "../src/db/migrate.ts";
import { listActiveSubscriptionsForAccount, upsertSubscription } from "../src/db/subscriptions.ts";
import type { AppDeps } from "../src/deps.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA,tokB:userB"));

const A = { authorization: "Bearer tokA", "content-type": "application/json" };
const B = { authorization: "Bearer tokB", "content-type": "application/json" };

function pullOctokit(): OctokitRequest {
  return {
    request: async (route: string): Promise<OctokitResponse> => {
      if (route.includes("/files")) return { status: 200, headers: {}, data: [] };
      return {
        status: 200,
        headers: { etag: 'W/"x"' },
        data: { number: 42, state: "open", title: "hi" },
      };
    },
  };
}

function deps(extra: Partial<AppDeps> = {}): AppDeps {
  const account = createAccount({ login: "alpha", token: undefined, octokit: pullOctokit() });
  return {
    db,
    auth,
    accountFor: (login) => (login === "alpha" ? account : undefined),
    ...extra,
  };
}

guarded("subscription management", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM subscriptions");
    await db.sql.unsafe("DELETE FROM documents");
    await db.sql.unsafe("DELETE FROM gh_accounts");
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: "x" });
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: "y" });
  });

  test("listActiveSubscriptionsForAccount returns only that account's active rows", async () => {
    await upsertSubscription(db, "alpha", "repo", "alpha/one");
    await upsertSubscription(db, "alpha", "repo", "alpha/two");
    await upsertSubscription(db, "beta", "repo", "beta/one");
    await db.sql.unsafe("UPDATE subscriptions SET active = false WHERE target = 'alpha/two'");

    const alpha = await listActiveSubscriptionsForAccount(db, "alpha");
    expect(alpha.map((s) => s.target)).toEqual(["alpha/one"]);

    const beta = await listActiveSubscriptionsForAccount(db, "beta");
    expect(beta.map((s) => s.target)).toEqual(["beta/one"]);

    expect(await listActiveSubscriptionsForAccount(db, "nobody")).toEqual([]);
  });

  test("subscribe by PR URL parses target, sets account_id, and triggers an immediate sync", async () => {
    const app = createApp(deps());
    const res = await app.request("/v1/subscriptions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ target: "https://github.com/alpha/repo/pull/42" }),
    });
    expect(res.status).toBe(201);
    const body = (await res.json()) as { kind: string; target: string; account: string };
    expect(body.kind).toBe("pull_request");
    expect(body.target).toBe("alpha/repo#42");
    expect(body.account).toBe("alpha");

    const [sub] = await db.sql<{ account_id: string | null }[]>`
      SELECT account_id::text FROM subscriptions WHERE target = 'alpha/repo#42'
    `;
    expect(sub?.account_id).not.toBeNull();

    const [doc] = await db.sql<{ n: string }[]>`
      SELECT count(*)::text AS n FROM documents WHERE kind = 'pull_request'
    `;
    expect(doc?.n).toBe("1");
  });

  test("subscribe accepts owner/repo#n shorthand", async () => {
    const app = createApp(deps());
    const res = await app.request("/v1/subscriptions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ kind: "pull_request", target: "alpha/repo#7", account: "alpha" }),
    });
    expect(res.status).toBe(201);
    expect(((await res.json()) as { target: string }).target).toBe("alpha/repo#7");
  });

  test("rejects an unparseable target", async () => {
    const app = createApp(deps());
    const res = await app.request("/v1/subscriptions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ target: "not a url" }),
    });
    expect(res.status).toBe(400);
  });

  test("CCT-776: rejects owner/repo path segments like ..", async () => {
    const app = createApp(deps());
    for (const target of ["../..#1", "https://github.com/../../x/pull/1"]) {
      const res = await app.request("/v1/subscriptions", {
        method: "POST",
        headers: A,
        body: JSON.stringify({ target }),
      });
      expect(res.status).toBe(400);
    }
  });

  test("cannot subscribe under an account the caller does not own", async () => {
    const app = createApp(deps());
    const res = await app.request("/v1/subscriptions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ target: "beta/repo#1", account: "beta" }),
    });
    expect(res.status).toBe(404);
  });

  test("list is owner-scoped; delete deactivates and is owner-checked", async () => {
    const app = createApp(deps());
    const created = await app.request("/v1/subscriptions", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ target: "alpha/repo#42" }),
    });
    const id = ((await created.json()) as { id: string }).id;

    const listA = await app.request("/v1/subscriptions", { headers: A });
    expect(((await listA.json()) as { items: unknown[] }).items.length).toBe(1);

    const listB = await app.request("/v1/subscriptions", { headers: B });
    expect(((await listB.json()) as { items: unknown[] }).items.length).toBe(0);

    const delByB = await app.request(`/v1/subscriptions/${id}`, { method: "DELETE", headers: B });
    expect(delByB.status).toBe(404);

    const delByA = await app.request(`/v1/subscriptions/${id}`, { method: "DELETE", headers: A });
    expect(delByA.status).toBe(204);

    const after = await app.request("/v1/subscriptions", { headers: A });
    expect(((await after.json()) as { items: unknown[] }).items.length).toBe(0);
  });

  test("CCT-775: /v1/status requires auth and scopes accounts to the caller", async () => {
    const app = createApp({
      db,
      auth,
      syncSnapshot: () => ({ last_run: null, accounts: ["alpha", "beta"] }),
    });
    expect((await app.request("/v1/status")).status).toBe(401);
    const res = await app.request("/v1/status", { headers: A });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { sync: { accounts: string[] } };
    expect(body.sync.accounts).toEqual(["alpha"]);
  });

  test("CCT-687: the permanent notification subscription cannot be deleted (400)", async () => {
    const app = createApp(deps());
    await upsertSubscription(db, "alpha", "notification", null, "notification");
    const [row] = await db.sql<{ id: string }[]>`
      SELECT id::text FROM subscriptions WHERE account = 'alpha' AND kind = 'notification'
    `;
    const id = row?.id as string;

    const del = await app.request(`/v1/subscriptions/${id}`, { method: "DELETE", headers: A });
    expect(del.status).toBe(400);
    expect(((await del.json()) as { error: { code: string } }).error.code).toBe(
      "permanent_subscription",
    );
  });
});
