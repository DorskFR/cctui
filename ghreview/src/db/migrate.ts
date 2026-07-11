import { readdir, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import type { DbHandle } from "./client.ts";

const MIGRATIONS_DIR = fileURLToPath(new URL("../../migrations/", import.meta.url));

export async function runMigrations(db: DbHandle): Promise<string[]> {
  const { sql, schema } = db;
  await sql.unsafe(`CREATE SCHEMA IF NOT EXISTS ${schema}`);
  await sql.unsafe(
    `CREATE TABLE IF NOT EXISTS ${schema}.schema_migrations (
       filename TEXT PRIMARY KEY,
       applied_at TIMESTAMPTZ NOT NULL DEFAULT now()
     )`,
  );

  const applied = new Set<string>(
    (
      await sql.unsafe<{ filename: string }[]>(`SELECT filename FROM ${schema}.schema_migrations`)
    ).map((r) => r.filename),
  );

  const files = (await readdir(MIGRATIONS_DIR))
    .filter((f) => f.endsWith(".sql") && !f.endsWith(".down.sql"))
    .sort();

  const ran: string[] = [];
  for (const file of files) {
    if (applied.has(file)) continue;
    const body = await readFile(new URL(file, `file://${MIGRATIONS_DIR}`), "utf8");
    await sql.begin(async (tx) => {
      await tx.unsafe(body).simple();
      await tx.unsafe(`INSERT INTO ${schema}.schema_migrations (filename) VALUES ($1)`, [file]);
    });
    ran.push(file);
  }
  return ran;
}
