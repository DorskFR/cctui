import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { randomBytes } from "node:crypto";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createSealer } from "../src/crypto/seal.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { runMigrations } from "../src/db/migrate.ts";
import { upsertSubscription } from "../src/db/subscriptions.ts";
import type { AppDeps } from "../src/deps.ts";
import type { PatValidator } from "../src/github/validate.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const sealer = createSealer(randomBytes(32).toString("base64"));
const auth = createStaticResolver(parseStaticTokens("tokA:userA,tokB:userB"));

const patToLogin: Record<string, string> = {
  "pat-alpha": "alpha",
  "pat-alpha2": "alpha",
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

guarded("github capability derivation", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string, "ghreview");
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM subscriptions; DELETE FROM gh_accounts");
  });

  test("available+not-enabled when the caller has no connectors", async () => {
    const app = createApp(appDeps());
    const res = await app.request("/v1/capabilities", { headers: A });
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({
      github: { available: true, enabled: false, repos: [] },
    });
  });

  test("enabled and repos reflect the caller's own connectors only", async () => {
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: sealer.seal("x") });
    await createGhAccount(db, { userId: "userB", login: "beta", encryptedPat: sealer.seal("y") });
    await upsertSubscription(db, "alpha", "repo", "alpha/one", "repo");
    await upsertSubscription(db, "alpha", "repo", "alpha/two", "repo");
    await upsertSubscription(db, "alpha", "pull_request", "alpha/one#1", null);
    await upsertSubscription(db, "beta", "repo", "beta/secret", "repo");

    const app = createApp(appDeps());
    const a = (await (await app.request("/v1/capabilities", { headers: A })).json()) as {
      github: { available: boolean; enabled: boolean; repos: string[] };
    };
    expect(a.github).toEqual({ available: true, enabled: true, repos: ["alpha/one", "alpha/two"] });

    const b = (await (await app.request("/v1/capabilities", { headers: B })).json()) as {
      github: { enabled: boolean; repos: string[] };
    };
    expect(b.github.enabled).toBe(true);
    expect(b.github.repos).toEqual(["beta/secret"]);
  });

  test("inactive repo subscriptions drop out of the tracked repo list", async () => {
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: sealer.seal("x") });
    await upsertSubscription(db, "alpha", "repo", "alpha/one", "repo");
    await db.sql`UPDATE subscriptions SET active = false WHERE target = 'alpha/one'`;

    const app = createApp(appDeps());
    const a = (await (await app.request("/v1/capabilities", { headers: A })).json()) as {
      github: { enabled: boolean; repos: string[] };
    };
    expect(a.github.enabled).toBe(true);
    expect(a.github.repos).toEqual([]);
  });
});

guarded("connector patch (update poll knobs + rotate PAT)", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string, "ghreview");
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM gh_accounts");
  });

  async function seedAlpha(): Promise<string> {
    const acct = await createGhAccount(db, {
      userId: "userA",
      login: "alpha",
      encryptedPat: sealer.seal("pat-alpha"),
    });
    return acct.id;
  }

  test("updates poll knobs and never returns a secret", async () => {
    const id = await seedAlpha();
    const app = createApp(appDeps());
    const res = await app.request(`/v1/accounts/${id}`, {
      method: "PATCH",
      headers: A,
      body: JSON.stringify({ poll_interval_ms: 60000, budget_ceiling: 0.5, rate_limit: 1000 }),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as Record<string, unknown>;
    expect(body.poll_interval_ms).toBe(60000);
    expect(body.budget_ceiling).toBe(0.5);
    expect(body.rate_limit).toBe(1000);
    expect(body).not.toHaveProperty("encrypted_pat");
    expect(body).not.toHaveProperty("token");
  });

  test("rotates the sealed PAT when a valid same-login token is supplied", async () => {
    const id = await seedAlpha();
    const app = createApp(appDeps());
    const res = await app.request(`/v1/accounts/${id}`, {
      method: "PATCH",
      headers: A,
      body: JSON.stringify({ token: "pat-alpha2" }),
    });
    expect(res.status).toBe(200);
    const [row] = await db.sql<{ encrypted_pat: string }[]>`
      SELECT encrypted_pat FROM gh_accounts WHERE login = 'alpha'
    `;
    expect(sealer.open(row?.encrypted_pat as string)).toBe("pat-alpha2");
  });

  test("rejects a token that resolves to a different login", async () => {
    const id = await seedAlpha();
    const app = createApp(appDeps());
    const res = await app.request(`/v1/accounts/${id}`, {
      method: "PATCH",
      headers: A,
      body: JSON.stringify({ token: "pat-beta" }),
    });
    expect(res.status).toBe(400);
    expect(((await res.json()) as { error: { code: string } }).error.code).toBe("login_mismatch");
  });

  test("is owner-scoped: another user cannot patch the connector", async () => {
    const id = await seedAlpha();
    const app = createApp(appDeps());
    const res = await app.request(`/v1/accounts/${id}`, {
      method: "PATCH",
      headers: B,
      body: JSON.stringify({ poll_interval_ms: 60000 }),
    });
    expect(res.status).toBe(404);
  });
});
