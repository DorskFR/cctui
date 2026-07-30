import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createStaticResolver, parseStaticTokens } from "../src/auth/resolver.ts";
import { createGhAccount } from "../src/db/accounts.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { runMigrations } from "../src/db/migrate.ts";
import type { AppDeps } from "../src/deps.ts";
import { EventBus } from "../src/events/bus.ts";
import { AccountManager } from "../src/sync/manager.ts";
import { deriveReviewDecision } from "../src/sync/pullEnrich.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const auth = createStaticResolver(parseStaticTokens("tokA:userA"));
const A = { authorization: "Bearer tokA", "content-type": "application/json" };

describe("AccountManager force sync", () => {
  test("retains conditional request state for the immediate poll", async () => {
    const conditionalState = { etag: 'W/"cached"', lastModified: "Tue, 14 Jul 2026 00:00:00 GMT" };
    const sql = Object.assign(
      async (strings: TemplateStringsArray) => {
        if (strings.join("").includes("UPDATE sync_state")) {
          conditionalState.etag = "";
          conditionalState.lastModified = "";
        }
        return [];
      },
      {
        unsafe: async () => [],
        begin: async () => [],
        listen: async () => ({ unlisten: async () => {} }),
      },
    );
    const manager = new AccountManager({
      db: { sql, close: async () => {} } as unknown as DbHandle,
      bus: new EventBus(),
      defaults: { pollIntervalMs: 60_000, budgetCeilingFraction: 0.2, rateLimitPerHour: 5_000 },
      open: (sealed) => sealed,
    });
    const statesSeenByPoll: (typeof conditionalState)[] = [];
    const managed = (
      manager as unknown as {
        managed: Map<string, { account: unknown; poller: { runOnce: () => Promise<void> } }>;
      }
    ).managed;
    managed.set("octocat", {
      account: {},
      poller: {
        runOnce: async () => {
          statesSeenByPoll.push({ ...conditionalState });
        },
      },
    });

    await expect(manager.forceSync("octocat")).resolves.toBe("ok");
    expect(statesSeenByPoll).toEqual([
      { etag: 'W/"cached"', lastModified: "Tue, 14 Jul 2026 00:00:00 GMT" },
    ]);
    expect(conditionalState.etag).toBe('W/"cached"');
  });

  test("reload rebuilds a poller when connector config changes, keeps it otherwise", async () => {
    let rows: Record<string, unknown>[] = [
      {
        id: "1",
        user_id: "userA",
        login: "octocat",
        poll_interval_ms: null,
        budget_ceiling: null,
        rate_limit: null,
        active: true,
        created_at: null,
        encrypted_pat: "sealed-1",
      },
    ];
    const sql = Object.assign(
      async (strings: TemplateStringsArray) =>
        strings.join("").includes("FROM gh_accounts") ? rows : [],
      {
        unsafe: () => "",
        begin: async () => [],
        listen: async () => ({ unlisten: async () => {} }),
      },
    );
    const manager = new AccountManager({
      db: { sql, close: async () => {} } as unknown as DbHandle,
      bus: new EventBus(),
      defaults: { pollIntervalMs: 60_000, budgetCeilingFraction: 0.2, rateLimitPerHour: 5_000 },
      open: (sealed) => sealed,
    });
    const internal = manager as unknown as {
      managed: Map<string, unknown>;
      reload: () => Promise<void>;
    };

    await internal.reload();
    const first = internal.managed.get("octocat");
    expect(first).toBeDefined();

    await internal.reload();
    expect(internal.managed.get("octocat")).toBe(first);

    rows = [{ ...rows[0], encrypted_pat: "sealed-2" }];
    await internal.reload();
    const second = internal.managed.get("octocat");
    expect(second).toBeDefined();
    expect(second).not.toBe(first);

    manager.stop();
  });
});

guarded("force sync route", () => {
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
    await createGhAccount(db, { userId: "userA", login: "alpha", encryptedPat: "x" });
  });

  function deps(result: "ok" | "busy" | "unknown"): AppDeps {
    return { db, auth, forceSync: async () => result };
  }

  test("forces a sync for the caller's single account", async () => {
    const app = createApp(deps("ok"));
    const res = await app.request("/v1/sync", { method: "POST", headers: A, body: "{}" });
    expect(res.status).toBe(200);
    const body = (await res.json()) as { account: string; status: string };
    expect(body).toEqual({ account: "alpha", status: "ok" });
  });

  test("returns 409 when a forced sync is already running", async () => {
    const app = createApp(deps("busy"));
    const res = await app.request("/v1/sync", { method: "POST", headers: A, body: "{}" });
    expect(res.status).toBe(409);
  });

  test("rejects an account the caller does not own", async () => {
    const app = createApp(deps("ok"));
    const res = await app.request("/v1/sync", {
      method: "POST",
      headers: A,
      body: JSON.stringify({ account: "beta" }),
    });
    expect(res.status).toBe(404);
  });
});

describe("deriveReviewDecision", () => {
  test("returns CHANGES_REQUESTED when any reviewer requested changes", () => {
    expect(deriveReviewDecision(["APPROVED", "CHANGES_REQUESTED"], 0)).toBe("CHANGES_REQUESTED");
  });

  test("returns REVIEW_REQUIRED when reviewers are still requested", () => {
    expect(deriveReviewDecision([], 1)).toBe("REVIEW_REQUIRED");
    expect(deriveReviewDecision(["COMMENTED"], 2)).toBe("REVIEW_REQUIRED");
  });

  test("returns APPROVED when approved and nothing pending", () => {
    expect(deriveReviewDecision(["APPROVED"], 0)).toBe("APPROVED");
  });

  test("returns null when there is no review signal", () => {
    expect(deriveReviewDecision([], 0)).toBeNull();
    expect(deriveReviewDecision(["COMMENTED"], 0)).toBeNull();
  });

  test("changes-requested wins over a pending re-request", () => {
    expect(deriveReviewDecision(["CHANGES_REQUESTED"], 1)).toBe("CHANGES_REQUESTED");
  });
});
