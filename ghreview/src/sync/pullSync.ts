import { getDocument, touchDocument, upsertDocument } from "../db/documents.ts";
import { clearSnoozeOnActivity } from "../db/prSnooze.ts";
import type { Subscription } from "../db/subscriptions.ts";
import { getSyncState } from "../db/syncState.ts";
import { conditionalRequest } from "../github/client.ts";
import {
  outcome,
  parsePullTarget,
  persistState,
  type SyncContext,
  type SyncOutcome,
  skipped,
} from "./context.ts";
import { removePull } from "./prune.ts";
import { enrichPullPayload, needsPullEnrichment } from "./pullEnrich.ts";
import { reconcilePullViewed } from "./viewedSync.ts";

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
  const existing = await getDocument(ctx.db, sub.account, "pull_request", key);
  const existingPayload =
    existing?.payload && typeof existing.payload === "object"
      ? (existing.payload as Record<string, unknown>)
      : null;
  if (res.status === 200 && res.data) {
    const pr = res.data as { state?: string; merged?: boolean; merged_at?: string | null };
    if (pr.state === "closed" || pr.merged === true || pr.merged_at != null) {
      await removePull(ctx.db, sub.account, owner, repo, number);
    } else {
      const payload = await enrichPullPayload(ctx.account.octokit, owner, repo, number, {
        ...existingPayload,
        ...(res.data as Record<string, unknown>),
      });
      await upsertDocument(ctx.db, {
        account: sub.account,
        kind: "pull_request",
        key,
        etag: res.etag,
        payload,
      });
      const updatedAtRaw = payload.updated_at;
      if (typeof updatedAtRaw === "string") {
        const activityAt = new Date(updatedAtRaw);
        if (!Number.isNaN(activityAt.getTime())) {
          await clearSnoozeOnActivity(ctx.db, sub.account, { owner, repo, number }, activityAt);
        }
      }
      await reconcilePullViewed(
        ctx.db,
        ctx.account,
        { owner, repo, number },
        res.data,
        ctx.syncViewedFromGithub ?? false,
      );
    }
  } else if (res.status === 304) {
    if (existingPayload && needsPullEnrichment(existingPayload)) {
      const payload = await enrichPullPayload(
        ctx.account.octokit,
        owner,
        repo,
        number,
        existingPayload,
      );
      await upsertDocument(ctx.db, {
        account: sub.account,
        kind: "pull_request",
        key,
        etag: existing?.etag ?? res.etag,
        payload,
      });
    } else {
      await touchDocument(ctx.db, sub.account, "pull_request", key);
    }
  }
  await persistState(ctx, sub, res);
  return outcome(res);
}
