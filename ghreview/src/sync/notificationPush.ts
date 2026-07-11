import type { DbHandle } from "../db/client.ts";
import { clearPushPending, listPendingReads, setPushError } from "../db/notificationState.ts";
import type { Account } from "../github/account.ts";
import { markThreadRead } from "../github/notifications.ts";

export interface PushOutcome {
  ok: boolean;
  error?: string;
}

export async function pushThreadRead(
  db: DbHandle,
  account: Account,
  threadId: string,
): Promise<PushOutcome> {
  const res = await markThreadRead(account.octokit, threadId);
  account.budget.record(res.status, res.rate);
  if (res.ok) {
    await clearPushPending(db, account.login, threadId);
    return { ok: true };
  }
  const error = `mark-as-read push failed with status ${res.status}`;
  await setPushError(db, account.login, threadId, error);
  return { ok: false, error };
}

export async function drainPendingReads(db: DbHandle, account: Account): Promise<void> {
  const pending = await listPendingReads(db, account.login);
  for (const p of pending) {
    if (!account.budget.canSpend()) break;
    await pushThreadRead(db, account, p.thread_id);
  }
}
