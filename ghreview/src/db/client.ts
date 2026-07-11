import postgres, { type Sql } from "postgres";

export type Db = Sql;

export interface DbHandle {
  sql: Db;
  schema: string;
  close: () => Promise<void>;
}

export function createDb(databaseUrl: string, schema: string): DbHandle {
  const sql = postgres(databaseUrl, {
    max: 10,
    onnotice: () => {},
    connection: { search_path: `${schema},public` },
  });
  return {
    sql,
    schema,
    close: () => sql.end({ timeout: 5 }),
  };
}
