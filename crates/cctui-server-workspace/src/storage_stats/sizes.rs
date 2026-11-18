use cctui_proto::api::{
    StorageBucket, StorageImages, StorageOverview, StorageSample, StorageSessionSize,
    StorageTableSize, StorageToolSize,
};
use sqlx::{PgConnection, PgPool};

use super::{SAMPLE_PAGES, SAMPLE_SEED, TOOL_RESULT, to_u64};

const TABLES: i64 = 25;
const TOP_SESSIONS: i64 = 20;
const TOP_TOOLS: i64 = 20;
/// How far back, in ids, a result looks for its call.
const CALL_WINDOW: i64 = 20_000;

pub const SIZE_BANDS: [&str; 5] = ["lt_1k", "k1_10", "k10_100", "k100_1m", "ge_1m"];
pub const AGE_BANDS: [&str; 5] = ["lt_1d", "d1_7", "d7_30", "d30_90", "ge_90d"];

/// Keeps a page load from spilling to disk or holding the pool on a full volume.
pub async fn bound(conn: &mut PgConnection) -> Result<(), sqlx::Error> {
    sqlx::raw_sql(
        "SET TRANSACTION READ ONLY; SET LOCAL statement_timeout = '10s'; \
         SET LOCAL work_mem = '32MB'; SET LOCAL max_parallel_workers_per_gather = 0",
    )
    .execute(conn)
    .await?;
    Ok(())
}

pub async fn sample_percent(conn: &mut PgConnection) -> Result<f64, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT LEAST(100::float8, GREATEST(0.01::float8, $1::float8 * 100 \
           / GREATEST(pg_relation_size('public.stream_events'::regclass)::float8 / 8192, 1)))",
    )
    .bind(SAMPLE_PAGES)
    .fetch_one(conn)
    .await
}

type TableRow = (String, i64, i64, i64, i64, Option<i64>);

async fn tables(pool: &PgPool) -> Result<Vec<StorageTableSize>, sqlx::Error> {
    let rows: Vec<TableRow> = sqlx::query_as(
        "SELECT c.relname::text, pg_total_relation_size(c.oid), pg_relation_size(c.oid), \
           pg_indexes_size(c.oid), \
           CASE WHEN c.reltoastrelid = 0 THEN 0 \
                ELSE pg_total_relation_size(c.reltoastrelid) END, \
           CASE WHEN c.reltuples < 0 THEN NULL ELSE c.reltuples::int8 END \
         FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE n.nspname = 'public' AND c.relkind IN ('r', 'p', 'm') \
         ORDER BY 2 DESC LIMIT $1",
    )
    .bind(TABLES)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(name, total, table, index, toast, rows)| StorageTableSize {
            name,
            total_bytes: to_u64(total),
            table_bytes: to_u64(table),
            index_bytes: to_u64(index),
            toast_bytes: to_u64(toast),
            row_estimate: rows.map(to_u64),
        })
        .collect())
}

async fn images(pool: &PgPool) -> Result<StorageImages, sqlx::Error> {
    let (blobs, bytes, table): (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), COALESCE(sum(byte_len), 0)::int8, \
           pg_total_relation_size('public.daemon_blobs'::regclass) FROM daemon_blobs",
    )
    .fetch_one(pool)
    .await?;
    Ok(StorageImages {
        blobs: to_u64(blobs),
        image_bytes: to_u64(bytes),
        table_bytes: to_u64(table),
    })
}

type SessionRow =
    (String, Option<String>, Option<String>, Option<String>, Option<String>, i64, i64, i64, i64);

async fn sessions(
    conn: &mut PgConnection,
    percent: f64,
) -> Result<(u64, Vec<StorageSessionSize>), sqlx::Error> {
    let sql = format!(
        "WITH s AS ( \
           SELECT e.session_id, pg_column_size(e.payload)::int8 AS bytes, {TOOL_RESULT} AS result \
           FROM stream_events e TABLESAMPLE SYSTEM ($1::real) {SAMPLE_SEED}), \
         top AS ( \
           SELECT session_id, count(*)::float8 AS events, sum(bytes)::float8 AS bytes, \
                  COALESCE(sum(bytes) FILTER (WHERE result), 0)::float8 AS tool_bytes \
           FROM s GROUP BY session_id ORDER BY 3 DESC LIMIT $3) \
         SELECT top.session_id, se.session_name, se.adapter_id, \
                COALESCE(m.display_name, m.name), m.kind, \
                round(top.events * $2)::int8, round(top.bytes * $2)::int8, \
                round(top.tool_bytes * $2)::int8, (SELECT count(*) FROM s)::int8 \
         FROM top LEFT JOIN sessions se ON se.id = top.session_id \
         LEFT JOIN machines m ON m.id = se.machine_uuid \
         ORDER BY top.bytes DESC"
    );
    let rows: Vec<SessionRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(percent)
        .bind(100.0 / percent)
        .bind(TOP_SESSIONS)
        .fetch_all(conn)
        .await?;
    let sampled = rows.first().map_or(0, |r| to_u64(r.8));
    let sessions = rows
        .into_iter()
        .map(|(session_id, name, harness, machine, machine_kind, events, bytes, tool, _)| {
            StorageSessionSize {
                session_id,
                name,
                harness,
                machine,
                machine_kind,
                events: to_u64(events),
                bytes: to_u64(bytes),
                tool_output_bytes: to_u64(tool),
            }
        })
        .collect();
    Ok((sampled, sessions))
}

async fn tools(conn: &mut PgConnection, percent: f64) -> Result<Vec<StorageToolSize>, sqlx::Error> {
    let sql = format!(
        "WITH r AS ( \
           SELECT e.id, e.session_id, pg_column_size(e.payload)::int8 AS bytes, \
                  COALESCE(e.payload->>'tool_use_id', e.payload->'payload'->>'call_id') AS ref \
           FROM stream_events e TABLESAMPLE SYSTEM ($1::real) {SAMPLE_SEED} WHERE {TOOL_RESULT}) \
         SELECT COALESCE(c.tool, ''), round(count(*) * $2)::int8, \
                round(sum(r.bytes)::float8 * $2)::int8 \
         FROM r LEFT JOIN LATERAL ( \
           SELECT COALESCE(k.payload->>'tool', k.payload->'payload'->>'name') AS tool \
           FROM stream_events k \
           WHERE r.ref IS NOT NULL AND k.session_id = r.session_id \
             AND k.id < r.id AND k.id >= r.id - $3 \
             AND (k.payload ? 'tool' \
                  OR k.payload->'payload'->>'type' IN ('function_call', 'custom_tool_call')) \
             AND COALESCE(k.payload->>'id', k.payload->>'tool_use_id', \
                          k.payload->'payload'->>'call_id') = r.ref \
           ORDER BY k.id DESC LIMIT 1) c ON true \
         GROUP BY 1 ORDER BY 3 DESC LIMIT $4"
    );
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(percent)
        .bind(100.0 / percent)
        .bind(CALL_WINDOW)
        .bind(TOP_TOOLS)
        .fetch_all(conn)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(tool, results, bytes)| StorageToolSize {
            tool,
            results: to_u64(results),
            bytes: to_u64(bytes),
        })
        .collect())
}

fn bands(keys: [&str; 5]) -> Vec<StorageBucket> {
    keys.into_iter().map(|k| StorageBucket { key: k.to_owned(), results: 0, bytes: 0 }).collect()
}

async fn buckets(
    conn: &mut PgConnection,
    percent: f64,
) -> Result<(Vec<StorageBucket>, Vec<StorageBucket>), sqlx::Error> {
    let sql = format!(
        "SELECT CASE WHEN bytes < 1024 THEN 0 WHEN bytes < 10240 THEN 1 \
                     WHEN bytes < 102400 THEN 2 WHEN bytes < 1048576 THEN 3 ELSE 4 END, \
                CASE WHEN created_at >= now() - interval '1 day' THEN 0 \
                     WHEN created_at >= now() - interval '7 days' THEN 1 \
                     WHEN created_at >= now() - interval '30 days' THEN 2 \
                     WHEN created_at >= now() - interval '90 days' THEN 3 ELSE 4 END, \
                round(count(*) * $2)::int8, round(sum(bytes)::float8 * $2)::int8 \
         FROM (SELECT e.created_at, pg_column_size(e.payload)::int8 AS bytes \
               FROM stream_events e TABLESAMPLE SYSTEM ($1::real) {SAMPLE_SEED} \
               WHERE {TOOL_RESULT}) s \
         GROUP BY 1, 2"
    );
    let rows: Vec<(i32, i32, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(percent)
        .bind(100.0 / percent)
        .fetch_all(conn)
        .await?;
    let (mut by_size, mut by_age) = (bands(SIZE_BANDS), bands(AGE_BANDS));
    for (size, age, results, bytes) in rows {
        for (list, band) in [(&mut by_size, size), (&mut by_age, age)] {
            if let Some(b) = usize::try_from(band).ok().and_then(|i| list.get_mut(i)) {
                b.results += to_u64(results);
                b.bytes += to_u64(bytes);
            }
        }
    }
    Ok((by_size, by_age))
}

async fn sample(pool: &PgPool) -> Result<StorageSample, sqlx::Error> {
    let mut tx = pool.begin().await?;
    bound(&mut tx).await?;
    let percent = sample_percent(&mut tx).await?;
    let (rows_sampled, sessions) = sessions(&mut tx, percent).await?;
    let tools = tools(&mut tx, percent).await?;
    let (by_size, by_age) = buckets(&mut tx, percent).await?;
    tx.rollback().await?;
    Ok(StorageSample { percent, rows_sampled, sessions, tools, by_size, by_age })
}

pub async fn overview(pool: &PgPool) -> Result<StorageOverview, sqlx::Error> {
    let database: i64 =
        sqlx::query_scalar("SELECT pg_database_size(current_database())").fetch_one(pool).await?;
    let tables = tables(pool).await?;
    let images = images(pool).await?;
    let (sample, sample_error) = match sample(pool).await {
        Ok(sample) => (Some(sample), None),
        Err(err) => {
            tracing::warn!(%err, "storage sample failed");
            (None, Some(err.to_string()))
        }
    };
    let lifecycle = super::info(pool, super::NAME).await?;
    Ok(StorageOverview {
        database_bytes: to_u64(database),
        tables,
        images,
        sample,
        sample_error,
        lifecycle,
    })
}
