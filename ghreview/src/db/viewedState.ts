import type { DbHandle } from "./client.ts";
import { EVENT_CHANNEL } from "./documents.ts";
import { accountOwnedBy } from "./notificationState.ts";

export interface ViewedStateItem {
  path: string;
  viewed: boolean;
  digest: string | null;
  push_pending: boolean;
  last_error: string | null;
  updated_at: string | null;
}

export interface PullRef {
  owner: string;
  repo: string;
  number: number;
}

interface StateRow {
  path: string;
  viewed: boolean;
  digest: string | null;
  push_pending: boolean;
  last_error: string | null;
  updated_at: string | null;
}

const SELECT_COLUMNS = `
  path, viewed, digest, push_pending, last_error,
  to_char(updated_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS updated_at
`;

function pullKey(ref: PullRef): string {
  return `${ref.owner}/${ref.repo}#${ref.number}`;
}

async function notifyViewedChange(db: DbHandle, account: string, ref: PullRef): Promise<void> {
  const notice = JSON.stringify({ account, kind: "pull_viewed", key: pullKey(ref) });
  await db.sql`SELECT pg_notify(${EVENT_CHANNEL}, ${notice})`;
}

export async function listViewedState(
  db: DbHandle,
  account: string,
  ref: PullRef,
  userId?: string,
): Promise<ViewedStateItem[]> {
  const { sql } = db;
  if (!userId || !(await accountOwnedBy(db, account, userId))) return [];
  return sql<StateRow[]>`
    SELECT ${sql.unsafe(SELECT_COLUMNS)}
    FROM viewed_state
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number}
    ORDER BY path
  `;
}

export async function applyViewedState(
  db: DbHandle,
  account: string,
  ref: PullRef,
  paths: string[],
  viewed: boolean,
  digestByPath: Map<string, string> = new Map(),
  userId?: string,
): Promise<ViewedStateItem[]> {
  const { sql } = db;
  if (!userId || !(await accountOwnedBy(db, account, userId))) return [];
  const items: ViewedStateItem[] = [];
  for (const path of paths) {
    const digest = digestByPath.get(path) ?? null;
    const [row] = await sql<StateRow[]>`
      INSERT INTO viewed_state (
        account, owner, repo, pull_number, path, viewed, digest,
        push_pending, last_error, updated_at
      ) VALUES (
        ${account}, ${ref.owner}, ${ref.repo}, ${ref.number}, ${path},
        ${viewed}, ${digest}, true, NULL, now()
      )
      ON CONFLICT (account, owner, repo, pull_number, path) DO UPDATE SET
        viewed = ${viewed},
        digest = ${viewed ? sql`COALESCE(${digest}, viewed_state.digest)` : sql`NULL`},
        push_pending = true,
        last_error = NULL,
        updated_at = now()
      RETURNING ${sql.unsafe(SELECT_COLUMNS)}
    `;
    if (row) items.push(row);
  }
  if (items.length > 0) await notifyViewedChange(db, account, ref);
  return items;
}

// Poll-in from github.com: must not clobber a local intent still owed a push,
// hence the push_pending = false guard in the UPDATE's WHERE.
export async function applyGithubViewedState(
  db: DbHandle,
  account: string,
  ref: PullRef,
  viewedByPath: Map<string, boolean>,
  digestByPath: Map<string, string> = new Map(),
): Promise<string[]> {
  const { sql } = db;
  const changed: string[] = [];
  for (const [path, viewed] of viewedByPath) {
    const digest = digestByPath.get(path) ?? null;
    // viewed=false only flips an existing local row — never inserts a not-viewed
    // row for every unviewed file, which would balloon the table.
    if (!viewed) {
      const [row] = await sql<{ path: string }[]>`
        UPDATE viewed_state
        SET viewed = false, digest = COALESCE(${digest}, digest), updated_at = now()
        WHERE account = ${account} AND owner = ${ref.owner} AND repo = ${ref.repo}
          AND pull_number = ${ref.number} AND path = ${path}
          AND push_pending = false AND viewed = true
        RETURNING path
      `;
      if (row) changed.push(row.path);
      continue;
    }
    const [row] = await sql<{ path: string }[]>`
      INSERT INTO viewed_state (
        account, owner, repo, pull_number, path, viewed, digest,
        push_pending, last_error, updated_at
      ) VALUES (
        ${account}, ${ref.owner}, ${ref.repo}, ${ref.number}, ${path},
        true, ${digest}, false, NULL, now()
      )
      ON CONFLICT (account, owner, repo, pull_number, path) DO UPDATE SET
        viewed = true,
        digest = COALESCE(${digest}, viewed_state.digest),
        updated_at = now()
      WHERE viewed_state.push_pending = false AND viewed_state.viewed IS DISTINCT FROM true
      RETURNING path
    `;
    if (row) changed.push(row.path);
  }
  if (changed.length > 0) await notifyViewedChange(db, account, ref);
  return changed;
}

// Mirror GitHub's reset: a file whose content changed vs the digest at mark
// time loses its viewed flag.
export async function invalidateChangedViewed(
  db: DbHandle,
  account: string,
  ref: PullRef,
  digestByPath: Map<string, string>,
): Promise<string[]> {
  const { sql } = db;
  const cleared: string[] = [];
  const rows = await sql<{ path: string; digest: string | null }[]>`
    SELECT path, digest FROM viewed_state
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number}
      AND viewed = true AND digest IS NOT NULL
  `;
  for (const row of rows) {
    const current = digestByPath.get(row.path);
    if (current === undefined || current === row.digest) continue;
    await sql`
      UPDATE viewed_state
      SET viewed = false, digest = ${current}, push_pending = false,
          last_error = NULL, updated_at = now()
      WHERE account = ${account} AND owner = ${ref.owner}
        AND repo = ${ref.repo} AND pull_number = ${ref.number} AND path = ${row.path}
    `;
    cleared.push(row.path);
  }
  if (cleared.length > 0) await notifyViewedChange(db, account, ref);
  return cleared;
}

export async function deleteViewedStateForPull(
  db: DbHandle,
  account: string,
  ref: PullRef,
): Promise<void> {
  await db.sql`
    DELETE FROM viewed_state
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number}
  `;
}

export interface PendingViewed extends PullRef {
  account: string;
  path: string;
  viewed: boolean;
}

export async function listPendingViewed(db: DbHandle, account: string): Promise<PendingViewed[]> {
  return db.sql<PendingViewed[]>`
    SELECT account, owner, repo, pull_number AS number, path, viewed
    FROM viewed_state
    WHERE account = ${account} AND push_pending = true
    ORDER BY updated_at
  `;
}

export async function clearViewedPushPending(
  db: DbHandle,
  account: string,
  ref: PullRef,
  path: string,
): Promise<void> {
  await db.sql`
    UPDATE viewed_state
    SET push_pending = false, last_error = NULL, updated_at = now()
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number} AND path = ${path}
  `;
}

export async function setViewedPushError(
  db: DbHandle,
  account: string,
  ref: PullRef,
  path: string,
  error: string,
): Promise<void> {
  await db.sql`
    UPDATE viewed_state
    SET last_error = ${error}, updated_at = now()
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number} AND path = ${path}
  `;
}
