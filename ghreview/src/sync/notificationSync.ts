import { upsertDocument } from "../db/documents.ts";
import { clearSnoozeOnActivity } from "../db/prSnooze.ts";
import { ensurePullSubscription, type Subscription } from "../db/subscriptions.ts";
import { getSyncState } from "../db/syncState.ts";
import { type ConditionalResult, conditionalRequest } from "../github/client.ts";
import { outcome, persistState, type SyncContext, type SyncOutcome } from "./context.ts";

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

interface NotificationThread {
  id: string;
  updated_at?: string;
  reason?: string;
  subject?: { type?: string; url?: string };
}

function parsePullApiUrl(url: string): { owner: string; repo: string; number: number } | null {
  const match = /\/repos\/([^/]+)\/([^/]+)\/pulls\/(\d+)(?:$|[/?#])/.exec(url);
  if (!match) return null;
  return { owner: match[1] as string, repo: match[2] as string, number: Number(match[3]) };
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
  if (thread.subject?.type === "PullRequest" && thread.subject.url) {
    const pr = parsePullApiUrl(thread.subject.url);
    if (pr) {
      const activityAt = thread.updated_at ? new Date(thread.updated_at) : null;
      if (activityAt && !Number.isNaN(activityAt.getTime())) {
        await clearSnoozeOnActivity(ctx.db, sub.account, pr, activityAt);
      }
      if (thread.reason && PARTICIPATING_REASONS.has(thread.reason)) {
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
