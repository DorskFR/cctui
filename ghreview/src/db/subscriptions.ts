import type { DbHandle } from "./client.ts";

export type SubscriptionKind = "repo" | "pull_request" | "notification";

export interface Subscription {
  id: string;
  account: string;
  kind: SubscriptionKind;
  target: string | null;
  active: boolean;
}

export async function upsertSubscription(
  db: DbHandle,
  account: string,
  kind: SubscriptionKind,
  target: string | null,
): Promise<void> {
  await db.sql`
    INSERT INTO subscriptions (account, kind, target, active)
    VALUES (${account}, ${kind}, ${target}, true)
    ON CONFLICT (account, kind, target) DO UPDATE SET active = true
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
