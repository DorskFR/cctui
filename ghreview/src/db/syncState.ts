import type { DbHandle } from "./client.ts";

export interface SyncState {
  account: string;
  kind: string;
  target: string;
  etag: string | null;
  cursor: string | null;
  last_modified: string | null;
  poll_interval_s: number | null;
  last_status: number | null;
  last_synced_at: Date | null;
}

export async function getSyncState(
  db: DbHandle,
  account: string,
  kind: string,
  target: string | null,
): Promise<SyncState | null> {
  const rows = await db.sql<SyncState[]>`
    SELECT account, kind, target, etag, cursor, last_modified, poll_interval_s,
           last_status, last_synced_at
    FROM sync_state
    WHERE account = ${account} AND kind = ${kind} AND target = ${target ?? ""}
    LIMIT 1
  `;
  return rows[0] ?? null;
}

export async function clearSyncEtags(db: DbHandle, account: string): Promise<void> {
  await db.sql`
    UPDATE sync_state
    SET etag = NULL, last_modified = NULL
    WHERE account = ${account}
  `;
}

export interface SyncStatePatch {
  etag?: string | null;
  cursor?: string | null;
  last_modified?: string | null;
  poll_interval_s?: number | null;
  last_status?: number | null;
  rate_limit?: number | null;
  rate_remaining?: number | null;
  rate_reset_at?: Date | null;
}

export async function saveSyncState(
  db: DbHandle,
  account: string,
  kind: string,
  target: string | null,
  patch: SyncStatePatch,
): Promise<void> {
  const t = target ?? "";
  await db.sql`
    INSERT INTO sync_state (
      account, kind, target, etag, cursor, last_modified, poll_interval_s,
      last_status, last_synced_at, rate_limit, rate_remaining, rate_reset_at
    ) VALUES (
      ${account}, ${kind}, ${t}, ${patch.etag ?? null}, ${patch.cursor ?? null},
      ${patch.last_modified ?? null}, ${patch.poll_interval_s ?? null},
      ${patch.last_status ?? null}, now(), ${patch.rate_limit ?? null},
      ${patch.rate_remaining ?? null}, ${patch.rate_reset_at ?? null}
    )
    ON CONFLICT (account, kind, target) DO UPDATE SET
      etag = COALESCE(EXCLUDED.etag, sync_state.etag),
      cursor = COALESCE(EXCLUDED.cursor, sync_state.cursor),
      last_modified = COALESCE(EXCLUDED.last_modified, sync_state.last_modified),
      poll_interval_s = COALESCE(EXCLUDED.poll_interval_s, sync_state.poll_interval_s),
      last_status = EXCLUDED.last_status,
      last_synced_at = now(),
      rate_limit = COALESCE(EXCLUDED.rate_limit, sync_state.rate_limit),
      rate_remaining = COALESCE(EXCLUDED.rate_remaining, sync_state.rate_remaining),
      rate_reset_at = COALESCE(EXCLUDED.rate_reset_at, sync_state.rate_reset_at)
  `;
}
