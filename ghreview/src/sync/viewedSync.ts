import type { DbHandle } from "../db/client.ts";
import {
  applyGithubViewedState,
  invalidateChangedViewed,
  type PullRef,
} from "../db/viewedState.ts";
import type { Account } from "../github/account.ts";
import { fetchGithubViewedState } from "../github/viewedFiles.ts";

interface PullFile {
  filename?: string;
  path?: string;
  sha?: string;
  patch?: string;
}

function hashString(input: string): string {
  let h = 0x811c9dc5;
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return (h >>> 0).toString(16).padStart(8, "0");
}

export function digestPullFiles(payload: unknown): Map<string, string> {
  const files = (payload as { files?: PullFile[] } | undefined)?.files;
  const out = new Map<string, string>();
  if (!Array.isArray(files)) return out;
  for (const f of files) {
    const path = f.path ?? f.filename;
    if (!path) continue;
    const digest = f.sha ?? (f.patch !== undefined ? `p:${hashString(f.patch)}` : null);
    if (digest) out.set(path, digest);
  }
  return out;
}

export async function reconcilePullViewed(
  db: DbHandle,
  account: Account,
  ref: PullRef,
  payload: unknown,
  pullFromGithub: boolean,
): Promise<void> {
  const digestByPath = digestPullFiles(payload);
  if (digestByPath.size > 0) {
    await invalidateChangedViewed(db, account.login, ref, digestByPath);
  }
  if (!pullFromGithub || !account.budget.canSpend()) return;
  try {
    const { viewedByPath } = await fetchGithubViewedState(
      account.graphql,
      ref.owner,
      ref.repo,
      ref.number,
    );
    account.budget.record(200, {});
    if (viewedByPath.size > 0) {
      await applyGithubViewedState(db, account.login, ref, viewedByPath, digestByPath);
    }
  } catch {}
}
