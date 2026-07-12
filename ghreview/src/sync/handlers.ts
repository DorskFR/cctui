import type { DbHandle } from "../db/client.ts";
import { touchDocument, upsertDocument } from "../db/documents.ts";
import type { Subscription, SubscriptionSource } from "../db/subscriptions.ts";
import { deactivateSubscription, upsertSubscription } from "../db/subscriptions.ts";
import { getSyncState, saveSyncState } from "../db/syncState.ts";
import type { Account } from "../github/account.ts";
import { type ConditionalResult, conditionalRequest } from "../github/client.ts";
import { reconcilePullViewed } from "./viewedSync.ts";

export interface SyncContext {
  db: DbHandle;
  account: Account;
  syncViewedFromGithub?: boolean;
}

export interface SyncOutcome {
  status: number;
  rate: ConditionalResult<unknown>["rate"];
  secondaryLimit: boolean;
  retryAfter: number | null;
  pollInterval: number | null;
}

function parseRepoTarget(target: string): { owner: string; repo: string } | null {
  const parts = target.split("/");
  if (parts.length !== 2 || !parts[0] || !parts[1]) return null;
  return { owner: parts[0], repo: parts[1] };
}

function parsePullTarget(target: string): { owner: string; repo: string; number: number } | null {
  const match = /^(.+?)\/(.+?)#(\d+)$/.exec(target);
  if (!match) return null;
  return { owner: match[1] as string, repo: match[2] as string, number: Number(match[3]) };
}

const PULL_FILES_PER_PAGE = 100;
const PULL_FILES_MAX_PAGES = 30;

const REPO_PULLS_PER_PAGE = 100;
const REPO_PULLS_MAX_PAGES = 30;

const PARTICIPATING_REASONS = new Set([
  "mention",
  "team_mention",
  "review_requested",
  "author",
  "assign",
  "comment",
]);

function parsePullApiUrl(url: string): { owner: string; repo: string; number: number } | null {
  const match = /\/repos\/([^/]+)\/([^/]+)\/pulls\/(\d+)(?:$|[/?#])/.exec(url);
  if (!match) return null;
  return { owner: match[1] as string, repo: match[2] as string, number: Number(match[3]) };
}

export async function fetchPullFiles(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
): Promise<unknown[]> {
  const files: unknown[] = [];
  for (let page = 1; page <= PULL_FILES_MAX_PAGES; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}/files", {
      owner,
      repo,
      pull_number: number,
      per_page: PULL_FILES_PER_PAGE,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as unknown[]) : [];
    files.push(...batch);
    if (batch.length < PULL_FILES_PER_PAGE) break;
  }
  return files;
}

function outcome(res: ConditionalResult<unknown>): SyncOutcome {
  return {
    status: res.status,
    rate: res.rate,
    secondaryLimit: res.secondaryLimit,
    retryAfter: res.retryAfter,
    pollInterval: res.pollInterval,
  };
}

export async function syncRepo(ctx: SyncContext, sub: Subscription): Promise<SyncOutcome> {
  const parsed = sub.target ? parseRepoTarget(sub.target) : null;
  if (!parsed) return skipped();
  const { owner, repo } = parsed;
  const state = await getSyncState(ctx.db, sub.account, "repo", sub.target);
  const res = await conditionalRequest(
    ctx.account.octokit,
    "GET /repos/{owner}/{repo}",
    { owner, repo },
    { etag: state?.etag ?? null },
  );
  const key = `${owner}/${repo}`;
  if (res.status === 200 && res.data) {
    await upsertDocument(ctx.db, {
      account: sub.account,
      kind: "repo",
      key,
      etag: res.etag,
      payload: res.data,
    });
  } else if (res.status === 304) {
    await touchDocument(ctx.db, sub.account, "repo", key);
  }
  await persistState(ctx, sub, res);
  if (ctx.account.budget.canSpend()) {
    await syncRepoPulls(ctx, sub, owner, repo);
  }
  return outcome(res);
}

interface OpenPull {
  number?: number;
}

async function syncRepoPulls(
  ctx: SyncContext,
  sub: Subscription,
  owner: string,
  repo: string,
): Promise<void> {
  const state = await getSyncState(ctx.db, sub.account, "repo_pulls", sub.target);
  let etag = state?.etag ?? null;
  let firstStatus: number | null = null;
  for (let page = 1; page <= REPO_PULLS_MAX_PAGES; page++) {
    if (!ctx.account.budget.canSpend()) break;
    const res = await conditionalRequest<OpenPull[]>(
      ctx.account.octokit,
      "GET /repos/{owner}/{repo}/pulls",
      { owner, repo, state: "open", per_page: REPO_PULLS_PER_PAGE, page },
      { etag: page === 1 ? etag : null },
    );
    ctx.account.budget.record(res.status, res.rate);
    if (res.secondaryLimit) ctx.account.budget.noteSecondaryLimit(res.retryAfter ?? undefined);
    if (page === 1) {
      firstStatus = res.status;
      etag = res.etag;
      if (res.status === 304) break;
    }
    if (res.status !== 200 || !Array.isArray(res.data)) break;
    for (const pr of res.data) {
      if (typeof pr?.number !== "number") continue;
      await ensurePullSubscription(ctx.db, sub.account, owner, repo, pr.number, "repo");
    }
    if (res.data.length < REPO_PULLS_PER_PAGE) break;
  }
  await saveSyncState(ctx.db, sub.account, "repo_pulls", sub.target, {
    etag,
    last_status: firstStatus,
  });
}

export async function syncPull(ctx: SyncContext, sub: Subscription): Promise<SyncOutcome> {
  const parsed = sub.target ? parsePullTarget(sub.target) : null;
  if (!parsed) return skipped();
  const { owner, repo, number } = parsed;
  const state = await getSyncState(ctx.db, sub.account, "pull_request", sub.target);
  const res = await conditionalRequest(
    ctx.account.octokit,
    "GET /repos/{owner}/{repo}/pulls/{pull_number}",
    { owner, repo, pull_number: number },
    { etag: state?.etag ?? null },
  );
  const key = `${owner}/${repo}#${number}`;
  if (res.status === 200 && res.data) {
    const files = await fetchPullFiles(ctx.account.octokit, owner, repo, number);
    await upsertDocument(ctx.db, {
      account: sub.account,
      kind: "pull_request",
      key,
      etag: res.etag,
      payload: { ...(res.data as Record<string, unknown>), files },
    });
    await reconcilePullViewed(
      ctx.db,
      ctx.account,
      { owner, repo, number },
      res.data,
      ctx.syncViewedFromGithub ?? false,
    );
    if ((res.data as { state?: string }).state === "closed") {
      await deactivateSubscription(ctx.db, sub.account, "pull_request", sub.target);
    }
  } else if (res.status === 304) {
    await touchDocument(ctx.db, sub.account, "pull_request", key);
  }
  await persistState(ctx, sub, res);
  return outcome(res);
}

interface NotificationThread {
  id: string;
  updated_at?: string;
  reason?: string;
  subject?: { type?: string; url?: string };
}

export async function syncNotifications(ctx: SyncContext, sub: Subscription): Promise<SyncOutcome> {
  const state = await getSyncState(ctx.db, sub.account, "notification", null);
  const res = await conditionalRequest<NotificationThread[]>(
    ctx.account.octokit,
    "GET /notifications",
    { all: false },
    { etag: state?.etag ?? null, lastModified: state?.last_modified ?? null },
  );
  if (res.status === 200 && Array.isArray(res.data)) {
    for (const thread of res.data) {
      if (!thread?.id) continue;
      await upsertDocument(ctx.db, {
        account: sub.account,
        kind: "notification",
        key: thread.id,
        etag: null,
        payload: thread,
      });
      if (
        thread.subject?.type === "PullRequest" &&
        thread.reason &&
        PARTICIPATING_REASONS.has(thread.reason) &&
        thread.subject.url
      ) {
        const pr = parsePullApiUrl(thread.subject.url);
        if (pr) {
          await ensurePullSubscription(
            ctx.db,
            sub.account,
            pr.owner,
            pr.repo,
            pr.number,
            "notification",
          );
        }
      }
    }
  }
  await persistState(ctx, sub, res);
  return outcome(res);
}

async function persistState(
  ctx: SyncContext,
  sub: Subscription,
  res: ConditionalResult<unknown>,
): Promise<void> {
  const resetAt = res.rate.reset ? new Date(res.rate.reset * 1000) : null;
  await saveSyncState(ctx.db, sub.account, sub.kind, sub.target, {
    etag: res.etag,
    last_modified: res.lastModified,
    poll_interval_s: res.pollInterval,
    last_status: res.status,
    rate_limit: res.rate.limit ?? null,
    rate_remaining: res.rate.remaining ?? null,
    rate_reset_at: resetAt,
  });
}

function skipped(): SyncOutcome {
  return { status: 0, rate: {}, secondaryLimit: false, retryAfter: null, pollInterval: null };
}

export async function ensurePullSubscription(
  db: DbHandle,
  account: string,
  owner: string,
  repo: string,
  number: number,
  source: SubscriptionSource | null = null,
): Promise<void> {
  await upsertSubscription(db, account, "pull_request", `${owner}/${repo}#${number}`, source);
}
