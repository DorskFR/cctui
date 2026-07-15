import type { DbHandle } from "./client.ts";

export type SubscriptionKind = "repo" | "pull_request" | "notification";

export interface Subscription {
  id: string;
  account: string;
  kind: SubscriptionKind;
  target: string | null;
  active: boolean;
}

export type SubscriptionSource = "user" | "repo" | "notification";

export async function upsertSubscription(
  db: DbHandle,
  account: string,
  kind: SubscriptionKind,
  target: string | null,
  source: SubscriptionSource | null = null,
): Promise<void> {
  await db.sql`
    INSERT INTO subscriptions (account, kind, target, active, source)
    VALUES (${account}, ${kind}, ${target}, true, ${source})
    ON CONFLICT (account, kind, target) DO UPDATE SET
      active = true,
      source = COALESCE(subscriptions.source, EXCLUDED.source)
  `;
}

export async function deactivateSubscription(
  db: DbHandle,
  account: string,
  kind: SubscriptionKind,
  target: string | null,
): Promise<void> {
  await db.sql`
    UPDATE subscriptions
    SET active = false
    WHERE account = ${account} AND kind = ${kind} AND target = ${target}
  `;
}

export async function listActiveSubscriptions(db: DbHandle): Promise<Subscription[]> {
  return db.sql<Subscription[]>`
    SELECT id::text, account, kind, target, active
    FROM subscriptions
    WHERE active = true
    ORDER BY id
  `;
}

export interface SubscriptionRow extends Subscription {
  created_at: string | null;
}

export async function upsertOwnedSubscription(
  db: DbHandle,
  userId: string,
  account: string,
  kind: SubscriptionKind,
  target: string | null,
): Promise<SubscriptionRow | null> {
  const { sql } = db;
  const rows = await sql<SubscriptionRow[]>`
    WITH owned AS (
      SELECT id FROM gh_accounts WHERE login = ${account} AND user_id = ${userId}
    )
    INSERT INTO subscriptions (account, account_id, kind, target, active)
    SELECT ${account}, owned.id, ${kind}, ${target}, true FROM owned
    ON CONFLICT (account, kind, target) DO UPDATE
      SET active = true, account_id = EXCLUDED.account_id
    RETURNING id::text, account, kind, target, active,
      to_char(created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
  `;
  return rows[0] ?? null;
}

export async function listSubscriptionsForUser(
  db: DbHandle,
  userId: string,
  account?: string,
): Promise<SubscriptionRow[]> {
  const { sql } = db;
  return sql<SubscriptionRow[]>`
    SELECT s.id::text, s.account, s.kind, s.target, s.active,
      to_char(s.created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
    FROM subscriptions s
    WHERE s.active = true
      AND EXISTS (
        SELECT 1 FROM gh_accounts ga
        WHERE ga.login = s.account AND ga.user_id = ${userId}
      )
      ${account ? sql`AND s.account = ${account}` : sql``}
    ORDER BY s.id
  `;
}

export async function listUserRepoSlugs(db: DbHandle, userId: string): Promise<string[]> {
  const { sql } = db;
  const rows = await sql<{ target: string }[]>`
    SELECT DISTINCT s.target
    FROM subscriptions s
    WHERE s.kind = 'repo' AND s.active = true AND s.target IS NOT NULL
      AND EXISTS (
        SELECT 1 FROM gh_accounts ga
        WHERE ga.login = s.account AND ga.user_id = ${userId}
      )
    ORDER BY s.target
  `;
  return rows.map((r) => r.target);
}

export async function getOwnedSubscriptionById(
  db: DbHandle,
  userId: string,
  id: string,
): Promise<SubscriptionRow | null> {
  const { sql } = db;
  const rows = await sql<SubscriptionRow[]>`
    SELECT s.id::text, s.account, s.kind, s.target, s.active,
      to_char(s.created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
    FROM subscriptions s
    WHERE s.id = ${id}
      AND EXISTS (
        SELECT 1 FROM gh_accounts ga
        WHERE ga.login = s.account AND ga.user_id = ${userId}
      )
    LIMIT 1
  `;
  return rows[0] ?? null;
}

export async function deactivateOwnedSubscription(
  db: DbHandle,
  userId: string,
  id: string,
): Promise<SubscriptionRow | null> {
  const { sql } = db;
  const rows = await sql<SubscriptionRow[]>`
    UPDATE subscriptions s
    SET active = false
    WHERE s.id = ${id}
      AND EXISTS (
        SELECT 1 FROM gh_accounts ga
        WHERE ga.login = s.account AND ga.user_id = ${userId}
      )
    RETURNING s.id::text, s.account, s.kind, s.target, s.active,
      to_char(s.created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
  `;
  return rows[0] ?? null;
}
