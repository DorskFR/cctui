import type { DbHandle } from "./client.ts";
import type { Envelope } from "./documents.ts";
import { EVENT_CHANNEL } from "./documents.ts";
import { accountOwnedBy } from "./notificationState.ts";
import type { PullRef } from "./viewedState.ts";

export interface SnoozedPull extends Envelope<"pull_request"> {
  owner: string;
  repo: string;
  number: number;
  snoozed_at: string;
}

function pullKey(ref: PullRef): string {
  return `${ref.owner}/${ref.repo}#${ref.number}`;
}

async function notifySnoozeChange(db: DbHandle, account: string, ref: PullRef): Promise<void> {
  const notice = JSON.stringify({ account, kind: "pull_snooze", key: pullKey(ref) });
  await db.sql`SELECT pg_notify(${EVENT_CHANNEL}, ${notice})`;
}

export async function snoozePull(
  db: DbHandle,
  account: string,
  ref: PullRef,
  userId?: string,
): Promise<boolean> {
  if (userId !== undefined && !(await accountOwnedBy(db, account, userId))) return false;
  await db.sql`
    INSERT INTO pr_snooze (account, owner, repo, pull_number, snoozed_at)
    VALUES (${account}, ${ref.owner}, ${ref.repo}, ${ref.number}, now())
    ON CONFLICT (account, owner, repo, pull_number) DO UPDATE SET snoozed_at = now()
  `;
  await notifySnoozeChange(db, account, ref);
  return true;
}

export async function unsnoozePull(
  db: DbHandle,
  account: string,
  ref: PullRef,
  userId?: string,
): Promise<boolean> {
  if (userId !== undefined && !(await accountOwnedBy(db, account, userId))) return false;
  const rows = await db.sql<{ account: string }[]>`
    DELETE FROM pr_snooze
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number}
    RETURNING account
  `;
  if (rows.length > 0) await notifySnoozeChange(db, account, ref);
  return rows.length > 0;
}

export async function isPullSnoozed(db: DbHandle, account: string, ref: PullRef): Promise<boolean> {
  const rows = await db.sql<{ ok: boolean }[]>`
    SELECT true AS ok FROM pr_snooze
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number}
    LIMIT 1
  `;
  return rows.length > 0;
}

export async function deleteSnoozeForPull(
  db: DbHandle,
  account: string,
  ref: PullRef,
): Promise<void> {
  await db.sql`
    DELETE FROM pr_snooze
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number}
  `;
}

export async function clearSnoozeOnActivity(
  db: DbHandle,
  account: string,
  ref: PullRef,
  activityAt: Date,
): Promise<boolean> {
  const rows = await db.sql<{ account: string }[]>`
    DELETE FROM pr_snooze
    WHERE account = ${account} AND owner = ${ref.owner}
      AND repo = ${ref.repo} AND pull_number = ${ref.number}
      AND snoozed_at < ${activityAt}
    RETURNING account
  `;
  if (rows.length > 0) await notifySnoozeChange(db, account, ref);
  return rows.length > 0;
}

export async function listSnoozedPulls(
  db: DbHandle,
  account?: string,
  userId?: string,
): Promise<SnoozedPull[]> {
  const { sql } = db;
  return sql<SnoozedPull[]>`
    SELECT
      d.account,
      d.kind,
      to_char(d.synced_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS synced_at,
      d.etag,
      d.payload,
      ps.owner,
      ps.repo,
      ps.pull_number AS number,
      to_char(ps.snoozed_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS snoozed_at
    FROM pr_snooze ps
    JOIN documents d
      ON d.account = ps.account
      AND d.kind = 'pull_request'
      AND d.key = ps.owner || '/' || ps.repo || '#' || ps.pull_number
    WHERE true
      ${account ? sql`AND ps.account = ${account}` : sql``}
      ${
        userId
          ? sql`AND EXISTS (SELECT 1 FROM gh_accounts ga
                 WHERE ga.login = ps.account AND ga.user_id = ${userId})`
          : sql``
      }
    ORDER BY ps.snoozed_at DESC
  `;
}
