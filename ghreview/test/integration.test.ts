import { afterAll, beforeAll, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { EVENT_CHANNEL, getDocument, listDocuments, upsertDocument } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import { upsertSubscription } from "../src/db/subscriptions.ts";
import { EventBus } from "../src/events/bus.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";
import { Poller } from "../src/sync/poller.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;

beforeAll(async () => {
  if (!DATABASE_URL) return;
  db = createDb(DATABASE_URL);
  await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
  await runMigrations(db);
});

afterAll(async () => {
  if (db) await db.close();
});

guarded("migrations", () => {
  test("are idempotent on a second run", async () => {
    const ran = await runMigrations(db);
    expect(ran).toEqual([]);
  });
});

guarded("documents store", () => {
  test("upsert emits NOTIFY only when the payload changes", async () => {
    const notices: string[] = [];
    const sub = await db.sql.listen(EVENT_CHANNEL, (p) => notices.push(p));

    const doc = {
      account: "acme",
      kind: "pull_request" as const,
      key: "acme/repo#1",
      etag: 'W/"v1"',
      payload: { title: "first", state: "open" },
    };
    expect(await upsertDocument(db, doc)).toBe(true);
    expect(await upsertDocument(db, doc)).toBe(false);
    expect(await upsertDocument(db, { ...doc, payload: { title: "edited", state: "open" } })).toBe(
      true,
    );

    await Bun.sleep(200);
    await sub.unlisten();
    expect(notices.length).toBe(2);
    expect(JSON.parse(notices[0] as string)).toEqual({
      account: "acme",
      kind: "pull_request",
      key: "acme/repo#1",
    });
  });

  test("reads back the stored envelope", async () => {
    const env = await getDocument(db, "acme", "pull_request", "acme/repo#1");
    expect(env?.payload).toEqual({ title: "edited", state: "open" });
    expect(env?.synced_at).toMatch(/^\d{4}-\d{2}-\d{2}T/);
  });

  test("paginates by keyset", async () => {
    for (let i = 0; i < 5; i++) {
      await upsertDocument(db, {
        account: "acme",
        kind: "repo",
        key: `acme/r${i}`,
        etag: null,
        payload: { i },
      });
    }
    const first = await listDocuments(db, "repo", { limit: 2 });
    expect(first.items.length).toBe(2);
    expect(first.next_cursor).not.toBeNull();
    const second = await listDocuments(db, "repo", { limit: 2, cursor: first.next_cursor ?? "" });
    expect(second.items.length).toBe(2);
    const firstKeys = first.items.map((e) => (e.payload as { i: number }).i);
    const secondKeys = second.items.map((e) => (e.payload as { i: number }).i);
    expect(firstKeys.some((k) => secondKeys.includes(k))).toBe(false);
  });
});

function warmMockOctokit(seen: Set<string>): OctokitRequest {
  const reset = String(Math.floor(Date.now() / 1000) + 3600);
  return {
    request: async (route, params = {}) => {
      const key = `${route}:${JSON.stringify(params)}`;
      const sig = `${route}:${(params as { pull_number?: number }).pull_number ?? ""}`;
      if (seen.has(sig)) {
        const err = { status: 304, response: { headers: { etag: 'W/"warm"' } } };
        throw err;
      }
      seen.add(sig);
      const res: OctokitResponse = {
        status: 200,
        headers: {
          etag: 'W/"warm"',
          "x-ratelimit-limit": "5000",
          "x-ratelimit-remaining": "4999",
          "x-ratelimit-reset": reset,
        },
        data: { number: (params as { pull_number?: number }).pull_number, warmed: key.length },
      };
      return res;
    },
  };
}

guarded("poller keeps tracked PRs warm", () => {
  test("50 PRs sync once, stay warm on re-poll with zero rate cost", async () => {
    await db.sql.unsafe(
      "DELETE FROM subscriptions WHERE account = 'warm'; DELETE FROM documents WHERE account = 'warm'",
    );
    for (let i = 1; i <= 50; i++) {
      await upsertSubscription(db, "warm", "pull_request", `warm/repo#${i}`);
    }
    const account = createAccount({
      login: "warm",
      token: undefined,
      octokit: warmMockOctokit(new Set()),
    });
    const bus = new EventBus();
    const poller = new Poller({ db, account, bus, intervalMs: 1_000 });

    await poller.runOnce();
    expect(account.budget.spent).toBe(50);

    const spentAfterFirst = account.budget.spent;
    await poller.runOnce();
    expect(account.budget.spent).toBe(spentAfterFirst);

    const page = await listDocuments(db, "pull_request", { account: "warm", limit: 100 });
    expect(page.items.length).toBe(50);
  });
});

guarded("read routes serve the store", () => {
  test("GET a synced pull returns the full envelope without network", async () => {
    await db.sql.unsafe(
      "INSERT INTO gh_accounts (user_id, login, encrypted_pat) VALUES ('__local__', 'warm', 'x') ON CONFLICT (login) DO NOTHING",
    );
    const app = createApp({ db, authDisabled: true });
    const res = await app.request("/v1/repos/warm/repo/pulls/1");
    expect(res.status).toBe(200);
    const body = (await res.json()) as { kind: string; payload: { number: number } };
    expect(body.kind).toBe("pull_request");
    expect(body.payload.number).toBe(1);
  });
});
