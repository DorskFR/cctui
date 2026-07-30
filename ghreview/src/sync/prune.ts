import type { DbHandle } from "../db/client.ts";
import { deleteDocument } from "../db/documents.ts";
import { deleteSnoozeForPull } from "../db/prSnooze.ts";
import { deleteReviewDraftsForPull } from "../db/reviewDrafts.ts";
import { deactivateSubscription } from "../db/subscriptions.ts";
import { deleteViewedStateForPull } from "../db/viewedState.ts";

export async function removePull(
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
  await deleteSnoozeForPull(db, account, ref);
  await deactivateSubscription(db, account, "pull_request", `${owner}/${repo}#${number}`);
}
