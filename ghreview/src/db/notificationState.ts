import type { DbHandle } from "./client.ts";
import type { Envelope } from "./documents.ts";
import { EVENT_CHANNEL } from "./documents.ts";

export interface NotificationState {
  read: boolean;
  done: boolean;
  archived: boolean;
  read_at: string | null;
  done_at: string | null;
  archived_at: string | null;
  push_pending: boolean;
  last_error: string | null;
  updated_at: string | null;
}

export interface NotificationStateItem {
  thread_id: string;
  state: NotificationState;
}

export interface InboxItem extends Envelope<"notification"> {
  state: NotificationState;
}

export interface InboxFilters {
  account?: string;
  reason?: string;
  repo?: string;
  unread?: boolean;
  undone?: boolean;
  archived?: boolean;
  since?: string;
  limit: number;
  cursor?: string;
  all?: boolean;
  userId?: string;
}

export interface InboxPage {
  items: InboxItem[];
  next_cursor: string | null;
}

const INBOX_ALL_HARD_CAP = 5000;

export interface StatePatch {
  read?: boolean;
  done?: boolean;
  archived?: boolean;
}

function encodeCursor(updatedAt: Date, key: string): string {
  return Buffer.from(`${updatedAt.toISOString()}|${key}`).toString("base64url");
}

function decodeCursor(cursor: string): { updatedAt: string; key: string } | null {
  try {
    const raw = Buffer.from(cursor, "base64url").toString("utf8");
    const sep = raw.indexOf("|");
    if (sep === -1) return null;
    return { updatedAt: raw.slice(0, sep), key: raw.slice(sep + 1) };
  } catch {
    return null;
  }
}

interface InboxRow {
  account: string;
  synced_at: string;
  etag: string | null;
  payload: unknown;
  cursor_updated_at: Date;
  cursor_key: string;
  s_read: boolean;
  s_done: boolean;
  s_archived: boolean;
  s_read_at: string | null;
  s_done_at: string | null;
  s_archived_at: string | null;
  s_push_pending: boolean;
  s_last_error: string | null;
  s_updated_at: string | null;
}

function rowState(r: InboxRow): NotificationState {
  return {
    read: r.s_read,
    done: r.s_done,
    archived: r.s_archived,
    read_at: r.s_read_at,
    done_at: r.s_done_at,
    archived_at: r.s_archived_at,
    push_pending: r.s_push_pending,
    last_error: r.s_last_error,
    updated_at: r.s_updated_at,
  };
}

export async function listNotificationInbox(
  db: DbHandle,
  filters: InboxFilters,
): Promise<InboxPage> {
  const { sql } = db;
  const all = filters.all ?? false;
  const decoded = !all && filters.cursor ? decodeCursor(filters.cursor) : null;
  const archived = filters.archived ?? false;
  const rows = await sql<InboxRow[]>`
    SELECT
      d.account,
      to_char(d.synced_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS synced_at,
      d.etag,
      d.payload,
      d.updated_at AS cursor_updated_at,
      d.key AS cursor_key,
      COALESCE(ns.read, false) AS s_read,
      COALESCE(ns.done, false) AS s_done,
      COALESCE(ns.archived, false) AS s_archived,
      to_char(ns.read_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS s_read_at,
      to_char(ns.done_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS s_done_at,
      to_char(ns.archived_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS s_archived_at,
      COALESCE(ns.push_pending, false) AS s_push_pending,
      ns.last_error AS s_last_error,
      to_char(ns.updated_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS s_updated_at
    FROM documents d
    LEFT JOIN notification_state ns
      ON ns.account = d.account AND ns.thread_id = d.key
    WHERE d.kind = 'notification'
      AND COALESCE(ns.archived, false) = ${archived}
      ${filters.account ? sql`AND d.account = ${filters.account}` : sql``}
      ${
        filters.userId
          ? sql`AND EXISTS (SELECT 1 FROM gh_accounts ga
                 WHERE ga.login = d.account AND ga.user_id = ${filters.userId})`
          : sql``
      }
      ${filters.reason ? sql`AND d.payload->>'reason' = ${filters.reason}` : sql``}
      ${filters.repo ? sql`AND d.payload->'repository'->>'full_name' = ${filters.repo}` : sql``}
      ${
        filters.unread === true
          ? sql`AND d.payload->>'unread' = 'true' AND COALESCE(ns.read, false) = false`
          : sql``
      }
      ${
        filters.unread === false
          ? sql`AND (d.payload->>'unread' = 'false' OR COALESCE(ns.read, false) = true)`
          : sql``
      }
      ${filters.undone === true ? sql`AND COALESCE(ns.done, false) = false` : sql``}
      ${filters.undone === false ? sql`AND COALESCE(ns.done, false) = true` : sql``}
      ${
        filters.since
          ? sql`AND (d.payload->>'updated_at')::timestamptz >= ${filters.since}::timestamptz`
          : sql``
      }
      ${
        decoded
          ? sql`AND (d.updated_at, d.key) < (${decoded.updatedAt}::timestamptz, ${decoded.key})`
          : sql``
      }
    ORDER BY d.updated_at DESC, d.key DESC
    ${all ? sql`LIMIT ${INBOX_ALL_HARD_CAP}` : sql`LIMIT ${filters.limit + 1}`}
  `;

  const hasMore = !all && rows.length > filters.limit;
  const pageRows = hasMore ? rows.slice(0, filters.limit) : rows;
  const last = pageRows.at(-1);
  const next_cursor =
    hasMore && last ? encodeCursor(last.cursor_updated_at, last.cursor_key) : null;

  const items: InboxItem[] = pageRows.map((r) => ({
    account: r.account,
    kind: "notification" as const,
    synced_at: r.synced_at,
    etag: r.etag,
    payload: r.payload,
    state: rowState(r),
  }));
  return { items, next_cursor };
}

interface StateRow {
  read: boolean;
  done: boolean;
  archived: boolean;
  read_at: string | null;
  done_at: string | null;
  archived_at: string | null;
  push_pending: boolean;
  last_error: string | null;
  updated_at: string | null;
}

export async function accountOwnedBy(
  db: DbHandle,
  account: string,
  userId: string,
): Promise<boolean> {
  const rows = await db.sql<{ ok: boolean }[]>`
    SELECT true AS ok FROM gh_accounts
    WHERE login = ${account} AND user_id = ${userId}
    LIMIT 1
  `;
  return rows.length > 0;
}

export async function applyNotificationState(
  db: DbHandle,
  account: string,
  threadIds: string[],
  patch: StatePatch,
  userId?: string,
): Promise<NotificationStateItem[]> {
  const { sql } = db;
  if (!userId || !(await accountOwnedBy(db, account, userId))) {
    return [];
  }
  const setRead = patch.read !== undefined;
  const setDone = patch.done !== undefined;
  const setArchived = patch.archived !== undefined;
  const readVal = patch.read === true;
  const doneVal = patch.done === true;
  const archivedVal = patch.archived === true;
  const wantsPush = setRead && readVal;

  const items: NotificationStateItem[] = [];
  for (const threadId of threadIds) {
    const [row] = await sql<StateRow[]>`
      INSERT INTO notification_state (
        account, thread_id, read, done, archived,
        read_at, done_at, archived_at, push_pending, last_error, updated_at
      ) VALUES (
        ${account}, ${threadId},
        ${setRead ? readVal : false},
        ${setDone ? doneVal : false},
        ${setArchived ? archivedVal : false},
        CASE WHEN ${setRead && readVal} THEN now() ELSE NULL END,
        CASE WHEN ${setDone && doneVal} THEN now() ELSE NULL END,
        CASE WHEN ${setArchived && archivedVal} THEN now() ELSE NULL END,
        ${wantsPush}, NULL, now()
      )
      ON CONFLICT (account, thread_id) DO UPDATE SET
        read = CASE WHEN ${setRead} THEN ${readVal} ELSE notification_state.read END,
        read_at = CASE WHEN ${setRead}
          THEN (CASE WHEN ${readVal} THEN now() ELSE NULL END)
          ELSE notification_state.read_at END,
        done = CASE WHEN ${setDone} THEN ${doneVal} ELSE notification_state.done END,
        done_at = CASE WHEN ${setDone}
          THEN (CASE WHEN ${doneVal} THEN now() ELSE NULL END)
          ELSE notification_state.done_at END,
        archived = CASE WHEN ${setArchived} THEN ${archivedVal} ELSE notification_state.archived END,
        archived_at = CASE WHEN ${setArchived}
          THEN (CASE WHEN ${archivedVal} THEN now() ELSE NULL END)
          ELSE notification_state.archived_at END,
        push_pending = CASE WHEN ${wantsPush} THEN true ELSE notification_state.push_pending END,
        last_error = CASE WHEN ${wantsPush} THEN NULL ELSE notification_state.last_error END,
        updated_at = now()
      RETURNING
        read, done, archived,
        to_char(read_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS read_at,
        to_char(done_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS done_at,
        to_char(archived_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS archived_at,
        push_pending, last_error,
        to_char(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
    `;
    if (!row) continue;
    items.push({ thread_id: threadId, state: row });
    await notifyStateChange(db, account, threadId);
  }
  return items;
}

async function notifyStateChange(db: DbHandle, account: string, threadId: string): Promise<void> {
  const notice = JSON.stringify({ account, kind: "notification_state", key: threadId });
  await db.sql`SELECT pg_notify(${EVENT_CHANNEL}, ${notice})`;
}

export async function getNotificationState(
  db: DbHandle,
  account: string,
  threadId: string,
): Promise<NotificationState | null> {
  const { sql } = db;
  const [row] = await sql<StateRow[]>`
    SELECT read, done, archived,
      to_char(read_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS read_at,
      to_char(done_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS done_at,
      to_char(archived_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS archived_at,
      push_pending, last_error,
      to_char(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
    FROM notification_state
    WHERE account = ${account} AND thread_id = ${threadId}
    LIMIT 1
  `;
  return row ?? null;
}

export interface PendingRead {
  account: string;
  thread_id: string;
}

export async function listPendingReads(db: DbHandle, account: string): Promise<PendingRead[]> {
  return db.sql<PendingRead[]>`
    SELECT account, thread_id
    FROM notification_state
    WHERE account = ${account} AND push_pending = true
    ORDER BY updated_at
  `;
}

export async function clearPushPending(
  db: DbHandle,
  account: string,
  threadId: string,
): Promise<void> {
  await db.sql`
    UPDATE notification_state
    SET push_pending = false, last_error = NULL, updated_at = now()
    WHERE account = ${account} AND thread_id = ${threadId}
  `;
}

export async function setPushError(
  db: DbHandle,
  account: string,
  threadId: string,
  error: string,
): Promise<void> {
  await db.sql`
    UPDATE notification_state
    SET last_error = ${error}, updated_at = now()
    WHERE account = ${account} AND thread_id = ${threadId}
  `;
}
