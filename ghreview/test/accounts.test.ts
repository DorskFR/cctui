import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createSealer } from "../src/crypto/seal.ts";
import { createGhAccount, deleteGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { upsertDocument } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import type { AppDeps } from "../src/deps.ts";
import type { PatValidator } from "../src/github/validate.ts";
import { dbGate } from "./dbGate.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = dbGate(describe, DATABASE_URL);

let db: DbHandle;
const sealer = createSealer(randomBytes(32).toString("base64"));
const auth = createStaticResolver(parseStaticTokens("tokA:userA,tokB:userB"));

const patToLogin: Record<string, string> = {
  "pat-alpha": "alpha",
  "pat-beta": "beta",
};
const validatePat: PatValidator = async (token) => {
  const login = patToLogin[token];
  return login ? { ok: true, login, status: 200 } : { ok: false, status: 401 };
};

function appDeps(extra: Partial<AppDeps> = {}): AppDeps {
  return { db, auth, sealer, validatePat, ...extra };
}

const A = { authorization: "Bearer tokA", "content-type": "application/json" };
const B = { authorization: "Bearer tokB", "content-type": "application/json" };

guarded("account CRUD", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM gh_accounts");
  });

  test("create validates the PAT, seals it, and never returns a secret", async () => {
    const app = createApp(appDeps());
    const res = await app.request("/v1/accounts", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ token: "pat-alpha" }),
    });
    expect(res.status).toBe(201);
    const body = (await res.json()) as Record<string, unknown>;
    expect(body.login).toBe("alpha");
    expect(JSON.stringify(body)).not.toContain("pat-alpha");
    expect(body).not.toHaveProperty("encrypted_pat");
    expect(body).not.toHaveProperty("token");

    const [row] = await db.sql<{ encrypted_pat: string }[]>`
      SELECT encrypted_pat FROM gh_accounts WHERE login = 'alpha'
    `;
    expect(row?.encrypted_pat).not.toContain("pat-alpha");
    expect(sealer.open(row?.encrypted_pat as string)).toBe("pat-alpha");
  });

  test("rejects an invalid PAT and a login mismatch", async () => {
    const app = createApp(appDeps());
    const bad = await app.request("/v1/accounts", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ token: "unknown" }),
    });
    expect(bad.status).toBe(400);

    const mismatch = await app.request("/v1/accounts", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ token: "pat-alpha", login: "notalpha" }),
    });
    expect(mismatch.status).toBe(400);
    expect(((await mismatch.json()) as { error: { code: string } }).error.code).toBe(
      "login_mismatch",
    );
  });

  test("list shows only the caller's accounts; delete is owner-scoped", async () => {
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: sealer.seal("x") });
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: sealer.seal("y") });
    const app = createApp(appDeps());

    const listA = await app.request("/v1/accounts", { headers: A });
    const bodyA = (await listA.json()) as { items: { login: string; id: string }[] };
    expect(bodyA.items.map((i) => i.login)).toEqual(["alpha"]);

    const alphaId = bodyA.items[0]?.id as string;
    const delByB = await app.request(`/v1/accounts/${alphaId}`, { method: "DELETE", headers: B });
    expect(delByB.status).toBe(404);

    const delByA = await app.request(`/v1/accounts/${alphaId}`, { method: "DELETE", headers: A });
    expect(delByA.status).toBe(204);
  });

  test("a login cannot be claimed by a second user", async () => {
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: sealer.seal("x") });
    const app = createApp(appDeps());
    const res = await app.request("/v1/accounts", {
      method: "POST",
      headers: B,
      body: JSON.stringify({ token: "pat-alpha" }),
    });
    expect(res.status).toBe(409);
  });
});

guarded("account deletion cascades all related resources", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe(
      `DELETE FROM documents; DELETE FROM sync_state; DELETE FROM notification_state;
       DELETE FROM viewed_state; DELETE FROM subscriptions; DELETE FROM review_draft_comments;
       DELETE FROM review_drafts; DELETE FROM gh_accounts`,
    );
  });

  test("deleting an account removes every login-keyed row, atomically", async () => {
    const acct = await createGhAccount(db, {
      userId: "userA",
      login: "alpha",
      encryptedPat: sealer.seal("x"),
    });
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: sealer.seal("y") });

    const { sql } = db;
    for (const login of ["alpha", "beta"]) {
      const acctRows = await sql<{ id: string }[]>`
        SELECT id::text FROM gh_accounts WHERE login = ${login}
      `;
      const accountId = acctRows[0]?.id as string;
      await sql`
        INSERT INTO documents (account, kind, key, payload)
        VALUES (${login}, 'pull_request', ${`${login}/repo#1`}, '{"number":1}')
      `;
      await sql`
        INSERT INTO sync_state (account, kind, target, etag)
        VALUES (${login}, 'notification', '', 'e1')
      `;
      await sql`
        INSERT INTO notification_state (account, thread_id, read)
        VALUES (${login}, 't1', true)
      `;
      await sql`
        INSERT INTO viewed_state (account, owner, repo, pull_number, path, viewed)
        VALUES (${login}, ${login}, 'repo', 1, 'a.ts', true)
      `;
      await sql`
        INSERT INTO subscriptions (account, kind, target, account_id)
        VALUES (${login}, 'repo', ${`${login}/repo`}, ${accountId})
      `;
      await sql`
        INSERT INTO subscriptions (account, kind, target, account_id)
        VALUES (${login}, 'pull', ${`${login}/repo#1`}, NULL)
      `;
      const draftRows = await sql<{ id: string }[]>`
        INSERT INTO review_drafts (account_id, account, owner, repo, pr_number)
        VALUES (${accountId}, ${login}, ${login}, 'repo', 1)
        RETURNING id::text
      `;
      const draftId = draftRows[0]?.id as string;
      await sql`
        INSERT INTO review_draft_comments (draft_id, path, line, body)
        VALUES (${draftId}, 'a.ts', 1, 'hi')
      `;
    }

    const removed = await deleteGhAccount(db, "userA", acct.id);
    expect(removed).toBe(true);

    const tables = [
      "documents",
      "sync_state",
      "notification_state",
      "viewed_state",
      "subscriptions",
      "review_drafts",
    ];
    for (const t of tables) {
      const rows = await sql<{ n: number }[]>`
        SELECT count(*)::int AS n FROM ${sql(t)} WHERE account = 'alpha'
      `;
      expect(rows[0]?.n).toBe(0);
    }
    const orphanComments = await sql<{ n: number }[]>`
      SELECT count(*)::int AS n FROM review_draft_comments
      WHERE draft_id NOT IN (SELECT id FROM review_drafts)
    `;
    expect(orphanComments[0]?.n).toBe(0);

    const betaDocs = await sql<{ n: number }[]>`
      SELECT count(*)::int AS n FROM documents WHERE account = 'beta'
    `;
    expect(betaDocs[0]?.n).toBe(1);
    const betaComments = await sql<{ n: number }[]>`
      SELECT count(*)::int AS n FROM review_draft_comments c
      JOIN review_drafts d ON d.id = c.draft_id WHERE d.account = 'beta'
    `;
    expect(betaComments[0]?.n).toBe(1);
  });

  test("deleting a non-owned account changes nothing and returns false", async () => {
    const acct = await createGhAccount(db, {
      userId: "userA",
      login: "alpha",
      encryptedPat: sealer.seal("x"),
    });
    const { sql } = db;
    await sql`
      INSERT INTO documents (account, kind, key, payload)
      VALUES ('alpha', 'repo', 'alpha/repo', '{}')
    `;
    const removed = await deleteGhAccount(db, "userB", acct.id);
    expect(removed).toBe(false);
    const rows = await sql<{ n: number }[]>`
      SELECT count(*)::int AS n FROM documents WHERE account = 'alpha'
    `;
    expect(rows[0]?.n).toBe(1);
  });
});

guarded("ownership isolation across users", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
    await db.sql.unsafe(
      "DELETE FROM documents; DELETE FROM notification_state; DELETE FROM gh_accounts",
    );
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: sealer.seal("x") });
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: sealer.seal("y") });
    await upsertDocument(db, {
      account: "alpha",
      kind: "repo",
      key: "alpha/repo",
      etag: null,
      payload: { full_name: "alpha/repo" },
    });
    await upsertDocument(db, {
      account: "beta",
      kind: "repo",
      key: "beta/repo",
      etag: null,
      payload: { full_name: "beta/repo" },
    });
    await upsertDocument(db, {
      account: "alpha",
      kind: "pull_request",
      key: "alpha/repo#1",
      etag: null,
      payload: { number: 1 },
    });
    await upsertDocument(db, {
      account: "beta",
      kind: "pull_request",
      key: "beta/repo#1",
      etag: null,
      payload: { number: 1 },
    });
    await upsertDocument(db, {
      account: "beta",
      kind: "notification",
      key: "bthread",
      etag: null,
      payload: {
        id: "bthread",
        reason: "mention",
        unread: true,
        updated_at: "2026-07-12T00:00:00Z",
      },
    });
  });
  afterAll(async () => {
    if (db) await db.close();
  });

  test("repo list is scoped to the caller's accounts", async () => {
    const app = createApp(appDeps());
    const a = (await (await app.request("/v1/repos", { headers: A })).json()) as {
      items: { account: string }[];
    };
    expect(a.items.map((i) => i.account)).toEqual(["alpha"]);
    const b = (await (await app.request("/v1/repos", { headers: B })).json()) as {
      items: { account: string }[];
    };
    expect(b.items.map((i) => i.account)).toEqual(["beta"]);
  });

  test("user A cannot fetch user B's repo or pull by direct URL", async () => {
    const app = createApp(appDeps());
    expect((await app.request("/v1/repos/beta/repo", { headers: A })).status).toBe(404);
    expect((await app.request("/v1/repos/beta/repo/pulls/1", { headers: A })).status).toBe(404);
    expect((await app.request("/v1/repos/beta/repo/pulls/1", { headers: B })).status).toBe(200);
  });

  test("notifications inbox is scoped by owner", async () => {
    const app = createApp(appDeps());
    const a = (await (await app.request("/v1/notifications", { headers: A })).json()) as {
      items: unknown[];
    };
    expect(a.items.length).toBe(0);
    const b = (await (await app.request("/v1/notifications", { headers: B })).json()) as {
      items: { account: string }[];
    };
    expect(b.items.map((i) => i.account)).toEqual(["beta"]);
  });

  test("user A cannot mutate user B's notification state", async () => {
    const app = createApp(appDeps());
    const res = await app.request("/v1/notifications/state", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "beta", thread_ids: ["bthread"], done: true }),
    });
    expect(res.status).toBe(200);
    expect(((await res.json()) as { items: unknown[] }).items.length).toBe(0);

    const [row] = await db.sql<{ done: boolean }[]>`
      SELECT done FROM notification_state WHERE account = 'beta' AND thread_id = 'bthread'
    `;
    expect(row).toBeUndefined();
  });
});
