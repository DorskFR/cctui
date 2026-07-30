import postgres, { type Sql } from "postgres";

export type Db = Sql;

export const GHREVIEW_SCHEMA = "ghreview";

export interface DbHandle {
  sql: Db;
  close: () => Promise<void>;
}

export function createDb(databaseUrl: string): DbHandle {
  const sql = postgres(databaseUrl, {
    max: 10,
    onnotice: () => {},
    connection: { search_path: `${GHREVIEW_SCHEMA},public` },
  });
  return {
    sql,
    close: () => sql.end({ timeout: 5 }),
  };
}
