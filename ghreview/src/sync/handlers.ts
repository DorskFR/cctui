import type { DbHandle } from "../db/client.ts";
import {
  deleteDocument,
  listPullDocumentNumbers,
  touchDocument,
  upsertDocument,
} from "../db/documents.ts";
import { deleteReviewDraftsForPull } from "../db/reviewDrafts.ts";
import type { Subscription, SubscriptionSource } from "../db/subscriptions.ts";
import { deactivateSubscription, upsertSubscription } from "../db/subscriptions.ts";
import { getSyncState, saveSyncState } from "../db/syncState.ts";
import { deleteViewedStateForPull } from "../db/viewedState.ts";
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

const PULL_COMMITS_PER_PAGE = 100;
const PULL_COMMITS_MAX_PAGES = 30;

const REPO_PULLS_PER_PAGE = 100;
const REPO_PULLS_MAX_PAGES = 30;

const NOTIFICATIONS_PER_PAGE = 100;
const NOTIFICATIONS_MAX_PAGES = 50;

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

interface PullFileStat {
  additions?: unknown;
  deletions?: unknown;
}

export interface PullStats {
  additions: number;
  deletions: number;
  changed_files: number;
}

export function pullStatsFromFiles(files: unknown[]): PullStats {
  let additions = 0;
  let deletions = 0;
  for (const f of files) {
    const stat = f as PullFileStat;
    if (typeof stat.additions === "number") additions += stat.additions;
    if (typeof stat.deletions === "number") deletions += stat.deletions;
  }
  return { additions, deletions, changed_files: files.length };
}

export function enrichPullStats(
  payload: Record<string, unknown>,
  files: unknown[],
): Record<string, unknown> {
  const stats = pullStatsFromFiles(files);
  return {
    ...payload,
    additions: typeof payload.additions === "number" ? payload.additions : stats.additions,
    deletions: typeof payload.deletions === "number" ? payload.deletions : stats.deletions,
    changed_files:
      typeof payload.changed_files === "number" ? payload.changed_files : stats.changed_files,
  };
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

export async function fetchPullCommits(
  octokit: Account["octokit"],
  owner: string,
  repo: string,
  number: number,
): Promise<unknown[]> {
  const commits: unknown[] = [];
  for (let page = 1; page <= PULL_COMMITS_MAX_PAGES; page++) {
    const res = await octokit.request("GET /repos/{owner}/{repo}/pulls/{pull_number}/commits", {
      owner,
      repo,
      pull_number: number,
      per_page: PULL_COMMITS_PER_PAGE,
      page,
    });
    const batch = Array.isArray(res.data) ? (res.data as unknown[]) : [];
    commits.push(...batch);
    if (batch.length < PULL_COMMITS_PER_PAGE) break;
  }
  return commits;
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
  const open = new Set<number>();
  let firstStatus: number | null = null;
  let walkedFull = false;
  for (let page = 1; page <= REPO_PULLS_MAX_PAGES; page++) {
    if (!ctx.account.budget.canSpend()) break;
    const res = await conditionalRequest<OpenPull[]>(
      ctx.account.octokit,
      "GET /repos/{owner}/{repo}/pulls",
      {
        owner,
        repo,
        state: "open",
        sort: "created",
        direction: "desc",
        per_page: REPO_PULLS_PER_PAGE,
        page,
      },
      {},
    );
    ctx.account.budget.record(res.status, res.rate);
    if (res.secondaryLimit) ctx.account.budget.noteSecondaryLimit(res.retryAfter ?? undefined);
    if (page === 1) firstStatus = res.status;
    if (res.status !== 200 || !Array.isArray(res.data)) break;
    for (const pr of res.data) {
      if (typeof pr?.number !== "number") continue;
      open.add(pr.number);
      await ensurePullSubscription(ctx.db, sub.account, owner, repo, pr.number, "repo");
    }
    if (!res.hasNextPage && res.data.length < REPO_PULLS_PER_PAGE) {
      walkedFull = true;
      break;
    }
  }
  if (walkedFull) await reconcileRepoPulls(ctx, sub.account, owner, repo, open);
  await saveSyncState(ctx.db, sub.account, "repo_pulls", sub.target, {
    last_status: firstStatus,
  });
}

async function reconcileRepoPulls(
  ctx: SyncContext,
  account: string,
  owner: string,
  repo: string,
  open: Set<number>,
): Promise<void> {
  const stored = await listPullDocumentNumbers(ctx.db, account, owner, repo);
  for (const number of stored) {
    if (open.has(number)) continue;
    await removePull(ctx.db, account, owner, repo, number);
  }
}

async function removePull(
  db: DbHandle,
  account: string,
  owner: string,
  repo: string,
  number: number,
): Promise<void> {
  const ref = { owner, repo, number };
  await deleteDocument(db, account, "pull_request", `${owner}/${repo}#${number}`);
  await deleteViewedStateForPull(db, account, ref);
  await deleteReviewDraftsForPull(db, account, ref);
  await deactivateSubscription(db, account, "pull_request", `${owner}/${repo}#${number}`);
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
    const pr = res.data as { state?: string; merged?: boolean; merged_at?: string | null };
    if (pr.state === "closed" || pr.merged === true || pr.merged_at != null) {
      await removePull(ctx.db, sub.account, owner, repo, number);
    } else {
      const files = await fetchPullFiles(ctx.account.octokit, owner, repo, number);
      const commits = await fetchPullCommits(ctx.account.octokit, owner, repo, number);
      const payload = enrichPullStats(res.data as Record<string, unknown>, files);
      payload.files = files;
      payload.commits_list = commits;
      await upsertDocument(ctx.db, {
        account: sub.account,
        kind: "pull_request",
        key,
        etag: res.etag,
        payload,
      });
      await reconcilePullViewed(
        ctx.db,
        ctx.account,
        { owner, repo, number },
        res.data,
        ctx.syncViewedFromGithub ?? false,
      );
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

async function ingestNotificationThread(
  ctx: SyncContext,
  sub: Subscription,
  thread: NotificationThread,
): Promise<void> {
  if (!thread?.id) return;
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

export async function syncNotifications(ctx: SyncContext, sub: Subscription): Promise<SyncOutcome> {
  const state = await getSyncState(ctx.db, sub.account, "notification", null);
  let firstRes: ConditionalResult<NotificationThread[]> | null = null;
  let etag = state?.etag ?? null;
  let lastModified = state?.last_modified ?? null;

  for (let page = 1; page <= NOTIFICATIONS_MAX_PAGES; page++) {
    if (!ctx.account.budget.canSpend()) break;
    const res = await conditionalRequest<NotificationThread[]>(
      ctx.account.octokit,
      "GET /notifications",
      { all: true, per_page: NOTIFICATIONS_PER_PAGE, page },
      page === 1 ? { etag, lastModified } : {},
    );
    ctx.account.budget.record(res.status, res.rate);
    if (res.secondaryLimit) ctx.account.budget.noteSecondaryLimit(res.retryAfter ?? undefined);
    if (page === 1) {
      firstRes = res;
      etag = res.etag;
      lastModified = res.lastModified;
      if (res.status === 304) break;
    }
    if (res.status !== 200 || !Array.isArray(res.data)) break;
    for (const thread of res.data) {
      await ingestNotificationThread(ctx, sub, thread);
    }
    // GitHub caps /notifications at 50/page regardless of per_page: a short page is not the last page.
    if (!res.hasNextPage) break;
  }

  const res = firstRes ?? {
    status: 0,
    etag,
    lastModified,
    pollInterval: null,
    retryAfter: null,
    secondaryLimit: false,
    rate: {},
    data: null,
    hasNextPage: false,
  };
  await persistState(ctx, sub, { ...res, etag, lastModified });
  return outcome({ ...res, etag, lastModified });
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
