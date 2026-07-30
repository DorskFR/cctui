import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createApp } from "../src/app.ts";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { EVENT_CHANNEL, upsertDocument } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import {
  applyNotificationState,
  getNotificationState,
  listNotificationInbox,
} from "../src/db/notificationState.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest, OctokitResponse } from "../src/github/client.ts";
import { markThreadRead } from "../src/github/notifications.ts";
import { drainPendingReads, pushThreadRead } from "../src/sync/notificationPush.ts";

describe("markThreadRead (transport)", () => {
  function mock(handler: (route: string) => OctokitResponse | never): {
    client: OctokitRequest;
    routes: string[];
  } {
    const routes: string[] = [];
    return {
      routes,
      client: {
        request: async (route) => {
          routes.push(route);
          return handler(route);
        },
      },
    };
  }

  test("205 Reset Content is ok and hits the thread PATCH route", async () => {
    const { client, routes } = mock(() => ({ status: 205, headers: {}, data: null }));
    const res = await markThreadRead(client, "thread-1");
    expect(res.ok).toBe(true);
    expect(routes[0]).toBe("PATCH /notifications/threads/{thread_id}");
  });

  test("404 (thread gone) is treated as ok", async () => {
    const { client } = mock(() => {
      throw { status: 404, response: { headers: {} } };
    });
    expect((await markThreadRead(client, "gone")).ok).toBe(true);
  });

  test("500 is a failure", async () => {
    const { client } = mock(() => {
      throw { status: 500, response: { headers: {} } };
    });
    const res = await markThreadRead(client, "boom");
    expect(res.ok).toBe(false);
    expect(res.status).toBe(500);
  });
});

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;

function notif(
  account: string,
  id: string,
  reason: string,
  repo: string,
  updatedAt: string,
  unread = true,
) {
  return upsertDocument(db, {
    account,
    kind: "notification",
    key: id,
    etag: null,
    payload: {
      id,
      reason,
      unread,
      updated_at: updatedAt,
      subject: { title: `${reason} on ${repo}`, type: "PullRequest" },
      repository: { full_name: repo },
    },
  });
}

function readingOctokit(status: number): OctokitRequest {
  return { request: async () => ({ status, headers: {}, data: null }) as OctokitResponse };
}

function failingOctokit(): OctokitRequest {
  return {
    request: async () => {
      throw { status: 500, response: { headers: {} } };
    },
  };
}

guarded("notification inbox + state", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string);
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
    await db.sql.unsafe(
      "INSERT INTO gh_accounts (user_id, login, encrypted_pat) VALUES ('__local__', 'nb', 'x') ON CONFLICT (login) DO NOTHING",
    );
  });

  afterAll(async () => {
    if (db) await db.close();
  });

  beforeEach(async () => {
    await db.sql.unsafe(
      "DELETE FROM notification_state WHERE account = 'nb'; DELETE FROM documents WHERE account = 'nb' AND kind = 'notification'",
    );
    await notif("nb", "t1", "review_requested", "DorskFR/cctui", "2026-07-10T00:00:00Z");
    await notif("nb", "t2", "mention", "DorskFR/other", "2026-07-11T00:00:00Z");
    await notif("nb", "t3", "ci_activity", "DorskFR/cctui", "2026-07-12T00:00:00Z");
  });

  test("filters by reason and repo", async () => {
    const byReason = await listNotificationInbox(db, {
      account: "nb",
      reason: "review_requested",
      limit: 30,
    });
    expect(byReason.items.map((i) => (i.payload as { id: string }).id)).toEqual(["t1"]);

    const byRepo = await listNotificationInbox(db, {
      account: "nb",
      repo: "DorskFR/cctui",
      limit: 30,
    });
    expect(byRepo.items.map((i) => (i.payload as { id: string }).id).sort()).toEqual(["t1", "t3"]);
  });

  test("filters by age via since", async () => {
    const recent = await listNotificationInbox(db, {
      account: "nb",
      since: "2026-07-11T00:00:00Z",
      limit: 30,
    });
    expect(recent.items.map((i) => (i.payload as { id: string }).id).sort()).toEqual(["t2", "t3"]);
  });

  test("all mode returns the full set with no cursor and ignores limit", async () => {
    const page = await listNotificationInbox(db, { account: "nb", limit: 1, all: true });
    expect(page.items.map((i) => (i.payload as { id: string }).id).sort()).toEqual([
      "t1",
      "t2",
      "t3",
    ]);
    expect(page.next_cursor).toBeNull();

    const capped = await listNotificationInbox(db, { account: "nb", limit: 1 });
    expect(capped.items.length).toBe(1);
    expect(capped.next_cursor).not.toBeNull();
  });

  test("bulk mark done+archived removes them from the default inbox", async () => {
    const before = await listNotificationInbox(db, { account: "nb", limit: 30 });
    expect(before.items.length).toBe(3);

    const items = await applyNotificationState(
      db,
      "nb",
      ["t1", "t3"],
      { done: true, archived: true },
      "__local__",
    );
    expect(items.length).toBe(2);
    expect(items.every((i) => i.state.done && i.state.archived)).toBe(true);

    const undone = await listNotificationInbox(db, { account: "nb", undone: true, limit: 30 });
    expect(undone.items.map((i) => (i.payload as { id: string }).id)).toEqual(["t2"]);

    const archived = await listNotificationInbox(db, { account: "nb", archived: true, limit: 30 });
    expect(archived.items.map((i) => (i.payload as { id: string }).id).sort()).toEqual([
      "t1",
      "t3",
    ]);
  });

  test("local state survives a payload re-sync", async () => {
    await applyNotificationState(db, "nb", ["t2"], { done: true, read: false }, "__local__");
    await notif("nb", "t2", "mention", "DorskFR/other", "2026-07-13T00:00:00Z", false);
    const state = await getNotificationState(db, "nb", "t2");
    expect(state?.done).toBe(true);
  });

  test("emits a notification_state NOTIFY on mutation", async () => {
    const notices: string[] = [];
    const sub = await db.sql.listen(EVENT_CHANNEL, (p) => notices.push(p));
    await applyNotificationState(db, "nb", ["t1"], { done: true }, "__local__");
    await Bun.sleep(200);
    await sub.unlisten();
    const parsed = notices.map((n) => JSON.parse(n) as { kind: string; key: string });
    expect(parsed.some((n) => n.kind === "notification_state" && n.key === "t1")).toBe(true);
  });

  test("mark-as-read pushes to GitHub and clears push_pending", async () => {
    await applyNotificationState(db, "nb", ["t1"], { read: true }, "__local__");
    let state = await getNotificationState(db, "nb", "t1");
    expect(state?.read).toBe(true);
    expect(state?.push_pending).toBe(true);

    const account = createAccount({ login: "nb", token: undefined, octokit: readingOctokit(205) });
    await pushThreadRead(db, account, "t1");
    state = await getNotificationState(db, "nb", "t1");
    expect(state?.push_pending).toBe(false);
    expect(state?.last_error).toBeNull();
  });

  test("push failure keeps state and retries on the next drain", async () => {
    await applyNotificationState(db, "nb", ["t3"], { read: true }, "__local__");
    const failing = createAccount({ login: "nb", token: undefined, octokit: failingOctokit() });
    await drainPendingReads(db, failing);
    let state = await getNotificationState(db, "nb", "t3");
    expect(state?.push_pending).toBe(true);
    expect(state?.last_error).toContain("500");

    const succeeding = createAccount({
      login: "nb",
      token: undefined,
      octokit: readingOctokit(205),
    });
    await drainPendingReads(db, succeeding);
    state = await getNotificationState(db, "nb", "t3");
    expect(state?.push_pending).toBe(false);
    expect(state?.last_error).toBeNull();
  });

  test("bulk state route pushes reads via accountFor", async () => {
    const account = createAccount({ login: "nb", token: undefined, octokit: readingOctokit(205) });
    const app = createApp({
      db,
      authDisabled: true,
      accountFor: (a) => (a === "nb" ? account : undefined),
    });
    const res = await app.request("/v1/notifications/state", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ account: "nb", thread_ids: ["t1", "t2"], read: true }),
    });
    expect(res.status).toBe(200);
    const body = (await res.json()) as {
      items: { thread_id: string; state: { read: boolean; push_pending: boolean } }[];
    };
    expect(body.items.length).toBe(2);
    expect(body.items.every((i) => i.state.read && !i.state.push_pending)).toBe(true);
  });

  test("single state route rejects an empty patch", async () => {
    const app = createApp({ db, authDisabled: true });
    const res = await app.request("/v1/notifications/t1/state", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ account: "nb" }),
    });
    expect(res.status).toBe(400);
    const body = (await res.json()) as { error: { code: string } };
    expect(body.error.code).toBe("invalid_request");
  });
});
