import { createHash } from "node:crypto";
import type { DbHandle } from "../db/client.ts";

export interface Principal {
  userId: string;
}

export interface AuthResolver {
  resolve: (token: string) => Promise<Principal | null>;
}

export function sha256Hex(value: string): string {
  return createHash("sha256").update(value).digest("hex");
}

export function createStaticResolver(tokens: Map<string, string>): AuthResolver {
  return {
    async resolve(token: string): Promise<Principal | null> {
      const userId = tokens.get(token);
      return userId ? { userId } : null;
    },
  };
}

export function parseStaticTokens(raw: string | undefined): Map<string, string> {
  const map = new Map<string, string>();
  if (!raw) return map;
  for (const pair of raw.split(",")) {
    const idx = pair.indexOf(":");
    if (idx === -1) continue;
    const token = pair.slice(0, idx).trim();
    const userId = pair.slice(idx + 1).trim();
    if (token && userId) map.set(token, userId);
  }
  return map;
}

export function createCctuiResolver(db: DbHandle, cctuiSchema: string): AuthResolver {
  return {
    async resolve(token: string): Promise<Principal | null> {
      const hash = sha256Hex(token);
      const { sql } = db;
      const rows = await sql<{ user_id: string }[]>`
        SELECT k.user_id::text AS user_id
        FROM ${sql(cctuiSchema)}.auth_keys k
        JOIN ${sql(cctuiSchema)}.users u ON u.id = k.user_id
        WHERE k.key_hash = ${hash}
          AND k.revoked_at IS NULL
          AND (k.expires_at IS NULL OR k.expires_at > now())
          AND u.revoked_at IS NULL AND u.disabled_at IS NULL
        LIMIT 1
      `;
      const row = rows[0];
      return row ? { userId: row.user_id } : null;
    },
  };
}
