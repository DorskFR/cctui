import type { DbHandle } from "./client.ts";

export interface GhAccount {
  id: string;
  user_id: string;
  login: string;
  poll_interval_ms: number | null;
  budget_ceiling: number | null;
  rate_limit: number | null;
  active: boolean;
  created_at: string | null;
}

export interface GhAccountWithSecret extends GhAccount {
  encrypted_pat: string;
}

export interface CreateAccountInput {
  userId: string;
  login: string;
  encryptedPat: string;
  pollIntervalMs?: number | null;
  budgetCeiling?: number | null;
  rateLimit?: number | null;
}

export interface UpdateAccountInput {
  pollIntervalMs?: number | null;
  budgetCeiling?: number | null;
  rateLimit?: number | null;
  encryptedPat?: string;
}

const PUBLIC_COLUMNS = `
  id::text, user_id, login, poll_interval_ms, budget_ceiling, rate_limit, active,
  to_char(created_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS created_at
`;

export async function createGhAccount(db: DbHandle, input: CreateAccountInput): Promise<GhAccount> {
  const { sql } = db;
  const [row] = await sql<GhAccount[]>`
    INSERT INTO gh_accounts (user_id, login, encrypted_pat, poll_interval_ms, budget_ceiling, rate_limit)
    VALUES (
      ${input.userId}, ${input.login}, ${input.encryptedPat},
      ${input.pollIntervalMs ?? null}, ${input.budgetCeiling ?? null}, ${input.rateLimit ?? null}
    )
    ON CONFLICT (login) DO UPDATE SET
      user_id = EXCLUDED.user_id,
      encrypted_pat = EXCLUDED.encrypted_pat,
      poll_interval_ms = EXCLUDED.poll_interval_ms,
      budget_ceiling = EXCLUDED.budget_ceiling,
      rate_limit = EXCLUDED.rate_limit,
      active = true,
      updated_at = now()
    WHERE gh_accounts.user_id = EXCLUDED.user_id
    RETURNING ${sql.unsafe(PUBLIC_COLUMNS)}
  `;
  if (!row) {
    throw new AccountConflictError(input.login);
  }
  return row;
}

export class AccountConflictError extends Error {
  constructor(login: string) {
    super(`GitHub account ${login} is already owned by another user`);
    this.name = "AccountConflictError";
  }
}

export async function listGhAccounts(db: DbHandle, userId: string): Promise<GhAccount[]> {
  const { sql } = db;
  return sql<GhAccount[]>`
    SELECT ${sql.unsafe(PUBLIC_COLUMNS)}
    FROM gh_accounts
    WHERE user_id = ${userId}
    ORDER BY login
  `;
}

export async function getGhAccount(
  db: DbHandle,
  userId: string,
  id: string,
): Promise<GhAccount | null> {
  const { sql } = db;
  const [row] = await sql<GhAccount[]>`
    SELECT ${sql.unsafe(PUBLIC_COLUMNS)}
    FROM gh_accounts
    WHERE user_id = ${userId} AND id = ${id}
    LIMIT 1
  `;
  return row ?? null;
}

export async function updateGhAccount(
  db: DbHandle,
  userId: string,
  id: string,
  patch: UpdateAccountInput,
): Promise<GhAccount | null> {
  const { sql } = db;
  return sql.begin(async (tx) => {
    const [cur] = await tx<GhAccountWithSecret[]>`
      SELECT ${tx.unsafe(PUBLIC_COLUMNS)}, encrypted_pat
      FROM gh_accounts
      WHERE user_id = ${userId} AND id = ${id}
      FOR UPDATE
    `;
    if (!cur) return null;
    const pollIntervalMs =
      patch.pollIntervalMs !== undefined ? patch.pollIntervalMs : cur.poll_interval_ms;
    const budgetCeiling =
      patch.budgetCeiling !== undefined ? patch.budgetCeiling : cur.budget_ceiling;
    const rateLimit = patch.rateLimit !== undefined ? patch.rateLimit : cur.rate_limit;
    const encryptedPat = patch.encryptedPat !== undefined ? patch.encryptedPat : cur.encrypted_pat;
    const [row] = await tx<GhAccount[]>`
      UPDATE gh_accounts SET
        poll_interval_ms = ${pollIntervalMs},
        budget_ceiling = ${budgetCeiling},
        rate_limit = ${rateLimit},
        encrypted_pat = ${encryptedPat},
        updated_at = now()
      WHERE id = ${id}
      RETURNING ${tx.unsafe(PUBLIC_COLUMNS)}
    `;
    return row ?? null;
  });
}

export async function countGhAccounts(db: DbHandle, userId: string): Promise<number> {
  const { sql } = db;
  const [row] = await sql<{ n: number }[]>`
    SELECT count(*)::int AS n FROM gh_accounts WHERE user_id = ${userId}
  `;
  return row?.n ?? 0;
}

export async function deleteGhAccount(db: DbHandle, userId: string, id: string): Promise<boolean> {
  const { sql } = db;
  return sql.begin(async (tx) => {
    const [owned] = await tx<{ login: string }[]>`
      SELECT login FROM gh_accounts
      WHERE user_id = ${userId} AND id = ${id}
      FOR UPDATE
    `;
    if (!owned) return false;
    const { login } = owned;
    await tx`DELETE FROM documents WHERE account = ${login}`;
    await tx`DELETE FROM sync_state WHERE account = ${login}`;
    await tx`DELETE FROM notification_state WHERE account = ${login}`;
    await tx`DELETE FROM viewed_state WHERE account = ${login}`;
    await tx`DELETE FROM subscriptions WHERE account = ${login}`;
    await tx`DELETE FROM review_drafts WHERE account = ${login}`;
    await tx`DELETE FROM gh_accounts WHERE id = ${id}`;
    return true;
  });
}

export async function listAllActiveAccounts(db: DbHandle): Promise<GhAccountWithSecret[]> {
  const { sql } = db;
  return sql<GhAccountWithSecret[]>`
    SELECT ${sql.unsafe(PUBLIC_COLUMNS)}, encrypted_pat
    FROM gh_accounts
    WHERE active = true
    ORDER BY id
  `;
}

export async function listUserLogins(db: DbHandle, userId: string): Promise<string[]> {
  const { sql } = db;
  const rows = await sql<{ login: string }[]>`
    SELECT login FROM gh_accounts WHERE user_id = ${userId}
  `;
  return rows.map((r) => r.login);
}
