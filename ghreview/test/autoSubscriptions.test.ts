import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { findDocument, listDocuments } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import { listActiveSubscriptions, upsertSubscription } from "../src/db/subscriptions.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest } from "../src/github/client.ts";
import { syncNotifications, syncPull, syncRepo } from "../src/sync/handlers.ts";

const DATABASE_URL = process.env.DATABASE_URL;
const guarded = DATABASE_URL ? describe : describe.skip;

let db: DbHandle;

function ctxFor(octokit: OctokitRequest) {
  const account = createAccount({ login: "auto", token: undefined, octokit });
  return { ctx: { db, account }, account };
}

async function sourceOf(target: string): Promise<string | null> {
  const [row] = await db.sql<{ source: string | null; active: boolean }[]>`
    SELECT source, active FROM subscriptions
    WHERE account = 'auto' AND kind = 'pull_request' AND target = ${target}
  `;
  return row?.source ?? null;
}

guarded("auto-subscription handlers", () => {
  beforeAll(async () => {
    db = createDb(DATABASE_URL as string, "ghreview");
    await db.sql.unsafe("DROP SCHEMA IF EXISTS ghreview CASCADE");
    await runMigrations(db);
  });
  afterAll(async () => {
    if (db) await db.close();
  });
  beforeEach(async () => {
    await db.sql.unsafe("DELETE FROM subscriptions");
    await db.sql.unsafe("DELETE FROM documents");
    await db.sql.unsafe("DELETE FROM sync_state");
  });

  test("CCT-657: participating PR notification auto-subscribes with source=notification", async () => {
    const threads = [
      {
        id: "t1",
        reason: "review_requested",
        subject: { type: "PullRequest", url: "https://api.github.com/repos/auto/repo/pulls/42" },
      },
      {
        id: "t2",
        reason: "subscribed",
        subject: { type: "PullRequest", url: "https://api.github.com/repos/auto/repo/pulls/7" },
      },
      {
        id: "t3",
        reason: "mention",
        subject: { type: "Issue", url: "https://api.github.com/repos/auto/repo/issues/9" },
      },
    ];
    const octokit: OctokitRequest = {
      request: async () => ({ status: 200, headers: {}, data: threads }),
    };
    const { ctx } = ctxFor(octokit);
    await syncNotifications(ctx, {
      id: "1",
      account: "auto",
      kind: "notification",
      target: null,
      active: true,
    });

    expect(await sourceOf("auto/repo#42")).toBe("notification");
    expect(await sourceOf("auto/repo#7")).toBeNull();
    const subs = await listActiveSubscriptions(db);
    expect(subs.filter((s) => s.kind === "pull_request").length).toBe(1);
  });

  test("CCT-687: syncNotifications requests all=true (full inbox: read + unread)", async () => {
    let seenAll: unknown;
    const octokit: OctokitRequest = {
      request: async (_route, params = {}) => {
        seenAll = (params as { all?: unknown }).all;
        return { status: 200, headers: {}, data: [] };
      },
    };
    const { ctx } = ctxFor(octokit);
    await syncNotifications(ctx, {
      id: "1",
      account: "auto",
      kind: "notification",
      target: null,
      active: true,
    });
    expect(seenAll).toBe(true);
  });

  test("CCT-675/687: paginates past the default and ingests every read+unread thread", async () => {
    const total = 230;
    const perPage = 100;
    let calls = 0;
    const octokit: OctokitRequest = {
      request: async (_route, params = {}) => {
        calls++;
        const page = Number((params as { page?: number }).page ?? 1);
        const start = (page - 1) * perPage;
        const batch = Array.from(
          { length: Math.max(0, Math.min(perPage, total - start)) },
          (_, i) => ({
            id: `n${start + i + 1}`,
            reason: "subscribed",
            unread: (start + i) % 2 === 0,
            subject: { type: "PullRequest" },
          }),
        );
        return { status: 200, headers: {}, data: batch };
      },
    };
    const { ctx } = ctxFor(octokit);
    await syncNotifications(ctx, {
      id: "1",
      account: "auto",
      kind: "notification",
      target: null,
      active: true,
    });

    expect(calls).toBe(3);
    const docs = await listDocuments(db, "notification", { account: "auto", limit: 1000 });
    expect(docs.items.length).toBe(total);
  });

  test("CCT-675: syncNotifications short-circuits on a 304 without walking pages", async () => {
    let calls = 0;
    const octokit: OctokitRequest = {
      request: async () => {
        calls++;
        throw { status: 304, response: { headers: { etag: 'W/"unchanged"' } } };
      },
    };
    const { ctx } = ctxFor(octokit);
    await syncNotifications(ctx, {
      id: "1",
      account: "auto",
      kind: "notification",
      target: null,
      active: true,
    });
    expect(calls).toBe(1);
  });

  test("CCT-656: syncRepo enumerates open PRs and subscribes with source=repo", async () => {
    const octokit: OctokitRequest = {
      request: async (route, params = {}) => {
        if (route === "GET /repos/{owner}/{repo}") {
          return { status: 200, headers: { etag: 'W/"r"' }, data: { full_name: "auto/repo" } };
        }
        const page = Number((params as { page?: number }).page ?? 1);
        const full = Array.from({ length: 100 }, (_, i) => ({ number: i + 1 }));
        const pages = [full, [{ number: 101 }]];
        return { status: 200, headers: { etag: 'W/"p1"' }, data: pages[page - 1] ?? [] };
      },
    };
    const { ctx } = ctxFor(octokit);
    await syncRepo(ctx, {
      id: "1",
      account: "auto",
      kind: "repo",
      target: "auto/repo",
      active: true,
    });

    const subs = await listActiveSubscriptions(db);
    const pulls = subs.filter((s) => s.kind === "pull_request");
    expect(pulls.length).toBe(101);
    expect(pulls.some((s) => s.target === "auto/repo#101")).toBe(true);
    expect(await sourceOf("auto/repo#1")).toBe("repo");
  });

  test("CCT-658: closed PR is deactivated, excluded from list, still directly fetchable", async () => {
    await upsertSubscription(db, "auto", "pull_request", "auto/repo#5", "user");
    const octokit: OctokitRequest = {
      request: async (route) => {
        if (route.includes("/files")) return { status: 200, headers: {}, data: [] };
        return {
          status: 200,
          headers: { etag: 'W/"c"' },
          data: { number: 5, state: "closed", merged: true, title: "done" },
        };
      },
    };
    const { ctx } = ctxFor(octokit);
    await syncPull(ctx, {
      id: "1",
      account: "auto",
      kind: "pull_request",
      target: "auto/repo#5",
      active: true,
    });

    const [row] = await db.sql<{ active: boolean }[]>`
      SELECT active FROM subscriptions WHERE target = 'auto/repo#5'
    `;
    expect(row?.active).toBe(false);

    const page = await listDocuments(db, "pull_request", { account: "auto", limit: 100 });
    expect(page.items.length).toBe(0);

    const doc = await findDocument(db, "pull_request", "auto/repo#5", { account: "auto" });
    expect((doc?.payload as { state?: string } | undefined)?.state).toBe("closed");
  });
});
