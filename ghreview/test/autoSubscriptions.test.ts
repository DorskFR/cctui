import { afterAll, beforeAll, beforeEach, describe, expect, test } from "bun:test";
import { createDb, type DbHandle } from "../src/db/client.ts";
import { findDocument, listDocuments } from "../src/db/documents.ts";
import { runMigrations } from "../src/db/migrate.ts";
import { listActiveSubscriptions, upsertSubscription } from "../src/db/subscriptions.ts";
import { createAccount } from "../src/github/account.ts";
import type { OctokitRequest } from "../src/github/client.ts";
import { syncNotifications } from "../src/sync/notificationSync.ts";
import { syncPull } from "../src/sync/pullSync.ts";
import { syncRepo } from "../src/sync/repoSync.ts";

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

  test("CCT-675/687: follows Link rel=next past GitHub's 50/page cap and ingests every read+unread thread", async () => {
    const total = 230;
    const cappedPerPage = 50;
    let calls = 0;
    const octokit: OctokitRequest = {
      request: async (_route, params = {}) => {
        calls++;
        const page = Number((params as { page?: number }).page ?? 1);
        const start = (page - 1) * cappedPerPage;
        const batch = Array.from(
          { length: Math.max(0, Math.min(cappedPerPage, total - start)) },
          (_, i) => ({
            id: `n${start + i + 1}`,
            reason: "subscribed",
            unread: (start + i) % 2 === 0,
            subject: { type: "PullRequest" },
          }),
        );
        const hasNext = start + batch.length < total;
        return {
          status: 200,
          headers: hasNext
            ? {
                link: `<https://api.github.com/notifications?all=true&per_page=100&page=${page + 1}>; rel="next", <https://api.github.com/notifications?all=true&per_page=100&page=5>; rel="last"`,
              }
            : {},
          data: batch,
        };
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

    expect(calls).toBe(5);
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

  test("CCT-694: merged PR is deleted (doc + viewed + draft) and deactivated on sync", async () => {
    await upsertSubscription(db, "auto", "pull_request", "auto/repo#5", "user");
    await db.sql`
      INSERT INTO documents (account, kind, key, etag, payload)
      VALUES ('auto', 'pull_request', 'auto/repo#5', NULL, ${db.sql.json({ number: 5, state: "open" })})
    `;
    await db.sql`
      INSERT INTO viewed_state (account, owner, repo, pull_number, path, viewed)
      VALUES ('auto', 'auto', 'repo', 5, 'a.ts', true)
    `;
    const [acct] = await db.sql<{ id: string }[]>`
      INSERT INTO gh_accounts (user_id, login, encrypted_pat)
      VALUES ('userA', 'auto', 'x') RETURNING id::text
    `;
    const accountId = acct?.id ?? "";
    await db.sql`
      INSERT INTO review_drafts (account_id, account, owner, repo, pr_number)
      VALUES (${accountId}, 'auto', 'auto', 'repo', 5)
    `;

    const octokit: OctokitRequest = {
      request: async (route) => {
        if (route.includes("/files")) return { status: 200, headers: {}, data: [] };
        return {
          status: 200,
          headers: { etag: 'W/"c"' },
          data: { number: 5, state: "closed", merged: true, merged_at: "2026-07-01T00:00:00Z" },
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
    expect(doc).toBeNull();

    const [viewed] = await db.sql<{ n: number }[]>`
      SELECT count(*)::int AS n FROM viewed_state WHERE account = 'auto' AND pull_number = 5
    `;
    expect(viewed?.n).toBe(0);

    const [drafts] = await db.sql<{ n: number }[]>`
      SELECT count(*)::int AS n FROM review_drafts WHERE account = 'auto' AND pr_number = 5
    `;
    expect(drafts?.n).toBe(0);
  });

  test("backfills missing files and commits when the parent pull is unchanged", async () => {
    await upsertSubscription(db, "auto", "pull_request", "auto/repo#8", "user");
    await db.sql`
      INSERT INTO documents (account, kind, key, etag, payload)
      VALUES ('auto', 'pull_request', 'auto/repo#8', 'W/"same"',
              ${db.sql.json({ number: 8, state: "open", head: { sha: "head-8" } })})
    `;
    await db.sql`
      INSERT INTO sync_state (account, kind, target, etag)
      VALUES ('auto', 'pull_request', 'auto/repo#8', 'W/"same"')
    `;
    const calls: string[] = [];
    const octokit: OctokitRequest = {
      request: async (route) => {
        calls.push(route);
        if (route.endsWith("/files")) {
          return {
            status: 200,
            headers: {},
            data: [{ filename: "a.ts", additions: 1, deletions: 0 }],
          };
        }
        if (route.endsWith("/commits")) {
          return { status: 200, headers: {}, data: [{ sha: "head-8" }] };
        }
        if (route.endsWith("/reviews")) {
          return { status: 200, headers: {}, data: [] };
        }
        throw { status: 304, response: { headers: { etag: 'W/"same"' } } };
      },
    };
    const { ctx } = ctxFor(octokit);
    await syncPull(ctx, {
      id: "1",
      account: "auto",
      kind: "pull_request",
      target: "auto/repo#8",
      active: true,
    });

    const doc = await findDocument(db, "pull_request", "auto/repo#8", { account: "auto" });
    expect(doc?.payload).toMatchObject({
      files: [{ filename: "a.ts" }],
      commits_list: [{ sha: "head-8" }],
      cctui_enriched_head_sha: "head-8",
    });
    expect(calls).toContain("GET /repos/{owner}/{repo}/pulls/{pull_number}/files");
    expect(calls).toContain("GET /repos/{owner}/{repo}/pulls/{pull_number}/commits");
  });

  test("CCT-694: syncRepoPulls reconciles PRs that merged between polls", async () => {
    for (const n of [1, 2, 3]) {
      await upsertSubscription(db, "auto", "pull_request", `auto/repo#${n}`, "repo");
      await db.sql`
        INSERT INTO documents (account, kind, key, etag, payload)
        VALUES ('auto', 'pull_request', ${`auto/repo#${n}`}, NULL,
                ${db.sql.json({ number: n, state: "open" })})
      `;
    }
    const octokit: OctokitRequest = {
      request: async (route) => {
        if (route === "GET /repos/{owner}/{repo}") {
          return { status: 200, headers: { etag: 'W/"r"' }, data: { full_name: "auto/repo" } };
        }
        return { status: 200, headers: {}, data: [{ number: 1 }, { number: 2 }] };
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

    expect(await findDocument(db, "pull_request", "auto/repo#3", { account: "auto" })).toBeNull();
    expect(
      await findDocument(db, "pull_request", "auto/repo#1", { account: "auto" }),
    ).not.toBeNull();
    expect(
      await findDocument(db, "pull_request", "auto/repo#2", { account: "auto" }),
    ).not.toBeNull();
    const [gone] = await db.sql<{ active: boolean }[]>`
      SELECT active FROM subscriptions WHERE target = 'auto/repo#3'
    `;
    expect(gone?.active).toBe(false);
  });

  test("CCT-694: syncRepoPulls follows Link rel=next so drafts past page 1 are enumerated", async () => {
    const total = 150;
    const perPage = 100;
    let calls = 0;
    const octokit: OctokitRequest = {
      request: async (route, params = {}) => {
        if (route === "GET /repos/{owner}/{repo}") {
          return { status: 200, headers: { etag: 'W/"r"' }, data: { full_name: "auto/repo" } };
        }
        calls++;
        const page = Number((params as { page?: number }).page ?? 1);
        const start = (page - 1) * perPage;
        const batch = Array.from(
          { length: Math.max(0, Math.min(perPage, total - start)) },
          (_, i) => ({ number: start + i + 1 }),
        );
        const hasNext = start + batch.length < total;
        return {
          status: 200,
          headers: hasNext
            ? { link: `<https://api.github.com/x?page=${page + 1}>; rel="next"` }
            : {},
          data: batch,
        };
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

    expect(calls).toBe(2);
    const subs = await listActiveSubscriptions(db);
    const pulls = subs.filter((s) => s.kind === "pull_request");
    expect(pulls.length).toBe(total);
    expect(pulls.some((s) => s.target === "auto/repo#150")).toBe(true);
  });
});
