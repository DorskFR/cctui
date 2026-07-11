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
