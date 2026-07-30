import type { DbHandle } from "../db/client.ts";
import type { Subscription } from "../db/subscriptions.ts";
import { saveSyncState } from "../db/syncState.ts";
import type { Account } from "../github/account.ts";
import type { ConditionalResult } from "../github/client.ts";

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

export function outcome(res: ConditionalResult<unknown>): SyncOutcome {
  return {
    status: res.status,
    rate: res.rate,
    secondaryLimit: res.secondaryLimit,
    retryAfter: res.retryAfter,
    pollInterval: res.pollInterval,
  };
}

export function skipped(): SyncOutcome {
  return { status: 0, rate: {}, secondaryLimit: false, retryAfter: null, pollInterval: null };
}

export async function persistState(
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

export function parseRepoTarget(target: string): { owner: string; repo: string } | null {
  const parts = target.split("/");
  if (parts.length !== 2 || !parts[0] || !parts[1]) return null;
  return { owner: parts[0], repo: parts[1] };
}

export function parsePullTarget(
  target: string,
): { owner: string; repo: string; number: number } | null {
  const match = /^(.+?)\/(.+?)#(\d+)$/.exec(target);
  if (!match) return null;
  return { owner: match[1] as string, repo: match[2] as string, number: Number(match[3]) };
}
