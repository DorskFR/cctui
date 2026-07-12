import type { DbHandle } from "../db/client.ts";
import { getDocument } from "../db/documents.ts";
import {
  clearViewedPushPending,
  listPendingViewed,
  type PullRef,
  setViewedPushError,
} from "../db/viewedState.ts";
import type { Account } from "../github/account.ts";
import { pushFileViewed, type ViewedPushResult } from "../github/viewedFiles.ts";

function pullKey(ref: PullRef): string {
  return `${ref.owner}/${ref.repo}#${ref.number}`;
}

export async function resolvePullNodeId(
  db: DbHandle,
  account: string,
  ref: PullRef,
): Promise<string | null> {
  const doc = await getDocument(db, account, "pull_request", pullKey(ref));
  const payload = doc?.payload as { node_id?: string } | undefined;
  return payload?.node_id ?? null;
}

export async function pushViewedFile(
  db: DbHandle,
  account: Account,
  ref: PullRef,
  path: string,
  viewed: boolean,
  nodeId: string | null,
): Promise<ViewedPushResult> {
  if (!nodeId) {
    const error = "viewed push skipped: pull request node id unknown";
    await setViewedPushError(db, account.login, ref, path, error);
    return { ok: false, error };
  }
  const res = await pushFileViewed(account.graphql, nodeId, path, viewed);
  if (res.ok) {
    await clearViewedPushPending(db, account.login, ref, path);
    return { ok: true };
  }
  await setViewedPushError(db, account.login, ref, path, res.error ?? "viewed push failed");
  return res;
}

export async function drainPendingViewed(db: DbHandle, account: Account): Promise<void> {
  const pending = await listPendingViewed(db, account.login);
  const nodeIds = new Map<string, string | null>();
  for (const p of pending) {
    if (!account.budget.canSpend()) break;
    const ref: PullRef = { owner: p.owner, repo: p.repo, number: p.number };
    const key = pullKey(ref);
    if (!nodeIds.has(key)) nodeIds.set(key, await resolvePullNodeId(db, account.login, ref));
    await pushViewedFile(db, account, ref, p.path, p.viewed, nodeIds.get(key) ?? null);
  }
}
