import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { listDocuments, upsertDocument } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import {
  clearSnoozeOnActivity,
  isPullSnoozed,
  listSnoozedPulls,
  snoozePull,
  unsnoozePull,
} from "../src/db/prSnooze.ts";
import { upsertSubscription } from "../src/db/subscriptions.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;
const ACCOUNT = "snz";
const REF = { owner: "DorskFR", repo: "cctui", number: 7 };
const KEY = `${REF.owner}/${REF.repo}#${REF.number}`;

async function seedPull(): Promise<void> {
  await upsertSubscription(db, ACCOUNT, "pull_request", KEY, "repo");
  await upsertDocument(db, {
    account: ACCOUNT,
    kind: "pull_request",
    key: KEY,
    etag: null,
    payload: { number: REF.number, title: "Snooze me", updated_at: "2026-07-10T00:00:00Z" },
  });
}

function listDefault() {
  return listDocuments(db, "pull_request", {
    account: ACCOUNT,
    keyPrefix: `${REF.owner}/`,
    limit: 50,
  });
}

guarded("pr snooze store", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
    await db.sql.unsafe(
      "INSERT INTO gh_accounts (user_id, login, encrypted_pat) VALUES ('u1', 'snz', 'x') ON CONFLICT (login) DO NOTHING",
    );
  });

  afterAll(async () => {
    if (db) await db.close();
  });

  beforeEach(async () => {
    await db.sql.unsafe(
      "DELETE FROM pr_snooze WHERE account = 'snz'; DELETE FROM subscriptions WHERE account = 'snz'; DELETE FROM documents WHERE account = 'snz'",
    );
    await seedPull();
  });

  test("snooze hides the PR from the default list and lists it as snoozed", async () => {
    expect((await listDefault()).items.length).toBe(1);

    const ok = await snoozePull(db, ACCOUNT, REF, "u1");
    expect(ok).toBe(true);
    expect(await isPullSnoozed(db, ACCOUNT, REF)).toBe(true);

    expect((await listDefault()).items.length).toBe(0);

    const snoozed = await listSnoozedPulls(db, ACCOUNT);
    expect(snoozed.length).toBe(1);
    const first = snoozed[0];
    expect(first).toMatchObject({ owner: REF.owner, repo: REF.repo, number: REF.number });
    expect((first?.payload as { title?: string })?.title).toBe("Snooze me");
  });

  test("un-snooze returns the PR to the default list", async () => {
    await snoozePull(db, ACCOUNT, REF, "u1");
    expect((await listDefault()).items.length).toBe(0);

    const removed = await unsnoozePull(db, ACCOUNT, REF, "u1");
    expect(removed).toBe(true);
    expect((await listDefault()).items.length).toBe(1);
    expect((await listSnoozedPulls(db, ACCOUNT)).length).toBe(0);
  });

  test("new activity newer than snooze auto-un-snoozes; stale activity does not", async () => {
    await snoozePull(db, ACCOUNT, REF, "u1");

    const stale = new Date(Date.now() - 60_000);
    expect(await clearSnoozeOnActivity(db, ACCOUNT, REF, stale)).toBe(false);
    expect(await isPullSnoozed(db, ACCOUNT, REF)).toBe(true);

    const fresh = new Date(Date.now() + 60_000);
    expect(await clearSnoozeOnActivity(db, ACCOUNT, REF, fresh)).toBe(true);
    expect(await isPullSnoozed(db, ACCOUNT, REF)).toBe(false);
    expect((await listDefault()).items.length).toBe(1);
  });

  test("ownership: a foreign user cannot snooze or list", async () => {
    expect(await snoozePull(db, ACCOUNT, REF, "intruder")).toBe(false);
    expect(await isPullSnoozed(db, ACCOUNT, REF)).toBe(false);

    await snoozePull(db, ACCOUNT, REF, "u1");
    expect((await listSnoozedPulls(db, ACCOUNT, "intruder")).length).toBe(0);
    expect((await listSnoozedPulls(db, ACCOUNT, "u1")).length).toBe(1);
  });
});
