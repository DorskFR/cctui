import type { DbHandle } from "./client.ts";

export const EVENT_CHANNEL = "ghreview_events";

export interface Envelope<K extends string = string> {
  account: string;
  kind: K;
  synced_at: string;
  etag: string | null;
  payload: unknown;
}

export interface UpsertInput {
  account: string;
  kind: string;
  key: string;
  etag: string | null;
  payload: unknown;
}

interface UpsertRow {
  synced_at: Date;
  updated_at: Date;
}

export async function upsertDocument(db: DbHandle, input: UpsertInput): Promise<boolean> {
  const { sql } = db;
  const [row] = await sql<UpsertRow[]>`
    INSERT INTO documents (account, kind, key, etag, payload, synced_at, updated_at)
    VALUES (${input.account}, ${input.kind}, ${input.key}, ${input.etag},
            ${sql.json(input.payload as never)}, now(), now())
    ON CONFLICT (account, kind, key) DO UPDATE SET
      etag = EXCLUDED.etag,
      payload = EXCLUDED.payload,
      synced_at = now(),
      updated_at = CASE
        WHEN documents.payload IS DISTINCT FROM EXCLUDED.payload THEN now()
        ELSE documents.updated_at
      END
    RETURNING synced_at, updated_at
  `;
  const changed = row !== undefined && row.updated_at.getTime() === row.synced_at.getTime();
  if (changed) {
    const notice = JSON.stringify({
      account: input.account,
      kind: input.kind,
      key: input.key,
    });
    await sql`SELECT pg_notify(${EVENT_CHANNEL}, ${notice})`;
  }
  return changed;
}

export async function deleteDocument(
  db: DbHandle,
  account: string,
  kind: string,
  key: string,
): Promise<boolean> {
  const { sql } = db;
  const rows = await sql<{ key: string }[]>`
    DELETE FROM documents
    WHERE account = ${account} AND kind = ${kind} AND key = ${key}
    RETURNING key
  `;
  if (rows.length === 0) return false;
  const notice = JSON.stringify({ account, kind, key });
  await sql`SELECT pg_notify(${EVENT_CHANNEL}, ${notice})`;
  return true;
}

export async function listPullDocumentNumbers(
  db: DbHandle,
  account: string,
  owner: string,
  repo: string,
): Promise<number[]> {
  const prefix = `${owner}/${repo}#`;
  const rows = await db.sql<{ key: string }[]>`
    SELECT key FROM documents
    WHERE account = ${account} AND kind = 'pull_request' AND key LIKE ${`${prefix}%`}
  `;
  const numbers: number[] = [];
  for (const row of rows) {
    const n = Number(row.key.slice(prefix.length));
    if (Number.isInteger(n)) numbers.push(n);
  }
  return numbers;
}

export async function touchDocument(
  db: DbHandle,
  account: string,
  kind: string,
  key: string,
): Promise<void> {
  await db.sql`
    UPDATE documents SET synced_at = now()
    WHERE account = ${account} AND kind = ${kind} AND key = ${key}
  `;
}

export async function getDocument<K extends string>(
  db: DbHandle,
  account: string,
  kind: K,
  key: string,
): Promise<Envelope<K> | null> {
  const rows = await db.sql<Envelope<K>[]>`
    SELECT account, kind, to_char(synced_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS synced_at,
           etag, payload
    FROM documents
    WHERE account = ${account} AND kind = ${kind} AND key = ${key}
    LIMIT 1
  `;
  return rows[0] ?? null;
}

export async function findDocument<K extends string>(
  db: DbHandle,
  kind: K,
  key: string,
  opts: { account?: string; userId?: string } = {},
): Promise<Envelope<K> | null> {
  const { sql } = db;
  const rows = await sql<Envelope<K>[]>`
    SELECT account, kind, to_char(synced_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS synced_at,
           etag, payload
    FROM documents
    WHERE kind = ${kind} AND key = ${key}
      ${opts.account ? sql`AND account = ${opts.account}` : sql``}
      ${
        opts.userId
          ? sql`AND EXISTS (SELECT 1 FROM gh_accounts ga
                 WHERE ga.login = documents.account AND ga.user_id = ${opts.userId})`
          : sql``
      }
    ORDER BY updated_at DESC
    LIMIT 1
  `;
  return rows[0] ?? null;
}

export interface Page<K extends string = string> {
  items: Envelope<K>[];
  next_cursor: string | null;
}

interface CursorRow<K extends string> extends Envelope<K> {
  cursor_updated_at: Date;
  cursor_key: string;
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

export interface ListOptions {
  account?: string;
  keyPrefix?: string;
  limit: number;
  cursor?: string;
  userId?: string;
}

export async function listDocuments<K extends string>(
  db: DbHandle,
  kind: K,
  opts: ListOptions,
): Promise<Page<K>> {
  const { sql } = db;
  const decoded = opts.cursor ? decodeCursor(opts.cursor) : null;
  const rows = await sql<CursorRow<K>[]>`
    SELECT account, kind, to_char(synced_at, 'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS synced_at,
           etag, payload, updated_at AS cursor_updated_at, key AS cursor_key
    FROM documents
    WHERE kind = ${kind}
      ${opts.account ? sql`AND account = ${opts.account}` : sql``}
      ${opts.keyPrefix ? sql`AND key LIKE ${`${opts.keyPrefix}%`}` : sql``}
      ${
        kind === "pull_request"
          ? sql`AND EXISTS (SELECT 1 FROM subscriptions s
                 WHERE s.account = documents.account AND s.kind = 'pull_request'
                   AND s.target = documents.key AND s.active = true)`
          : sql``
      }
      ${
        opts.userId
          ? sql`AND EXISTS (SELECT 1 FROM gh_accounts ga
                 WHERE ga.login = documents.account AND ga.user_id = ${opts.userId})`
          : sql``
      }
      ${
        decoded
          ? sql`AND (updated_at, key) < (${decoded.updatedAt}::timestamptz, ${decoded.key})`
          : sql``
      }
    ORDER BY updated_at DESC, key DESC
    LIMIT ${opts.limit + 1}
  `;

  const hasMore = rows.length > opts.limit;
  const pageRows = hasMore ? rows.slice(0, opts.limit) : rows;
  const last = pageRows.at(-1);
  const next_cursor =
    hasMore && last ? encodeCursor(last.cursor_updated_at, last.cursor_key) : null;

  const items: Envelope<K>[] = pageRows.map((r) => ({
    account: r.account,
    kind: r.kind,
    synced_at: r.synced_at,
    etag: r.etag,
    payload: r.payload,
  }));
  return { items, next_cursor };
}
