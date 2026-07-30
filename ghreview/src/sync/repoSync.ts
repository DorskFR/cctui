import { listPullDocumentNumbers, touchDocument, upsertDocument } from "../db/documents.ts";
import { ensurePullSubscription, type Subscription } from "../db/subscriptions.ts";
import { getSyncState, saveSyncState } from "../db/syncState.ts";
import { conditionalRequest } from "../github/client.ts";
import {
  outcome,
  parseRepoTarget,
  persistState,
  type SyncContext,
  type SyncOutcome,
  skipped,
} from "./context.ts";
import { removePull } from "./prune.ts";

const REPO_PULLS_PER_PAGE = 100;
const REPO_PULLS_MAX_PAGES = 30;

interface OpenPull {
  number?: number;
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
