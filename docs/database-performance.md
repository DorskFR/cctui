# Reading Postgres performance on the cctui database

`pg_stat_statements` is loaded via `shared_preload_libraries` on the
`cctui-database` deployment. The extension itself still has to exist in the
database:

```sql
CREATE EXTENSION IF NOT EXISTS pg_stat_statements;
```

These are the three queries the CCT-1057 investigation ran. Use them in that
order: total time finds what the database actually spends its day on, mean time
finds what a user feels, index usage finds what is being paid for and not used.

## 1. Where the time goes (total)

The statement at the top here is the one worth fixing, even if each call looks
cheap — a 20 ms query run every second outranks a 2 s report run twice a day.

```sql
SELECT calls,
       round(total_exec_time::numeric, 1) AS total_ms,
       round(mean_exec_time::numeric, 1)  AS mean_ms,
       round((shared_blks_hit + shared_blks_read)::numeric / calls, 0) AS blks_per_call,
       left(query, 120) AS query
FROM pg_stat_statements
ORDER BY total_exec_time DESC
LIMIT 20;
```

## 2. What feels slow (mean)

`calls > 20` keeps one-off maintenance statements and migrations out of the
list.

```sql
SELECT calls,
       round(mean_exec_time::numeric, 1) AS mean_ms,
       round(max_exec_time::numeric, 1)  AS max_ms,
       left(query, 120) AS query
FROM pg_stat_statements
WHERE calls > 20
ORDER BY mean_exec_time DESC
LIMIT 20;
```

A mean far below the max means bursts rather than a uniformly slow query — that
is the signature of a GIN pending-list flush landing on whichever backend
crosses `gin_pending_list_limit`, not of a bad plan.

## 3. What the indexes cost and return

```sql
SELECT relname, indexrelname,
       pg_size_pretty(pg_relation_size(indexrelid)) AS size,
       idx_scan,
       last_idx_scan
FROM pg_stat_user_indexes
ORDER BY pg_relation_size(indexrelid) DESC
LIMIT 25;
```

A large index with a near-zero `idx_scan` is a write tax with no reader. Before
dropping one, confirm no query *could* use it: a partial or expression index is
only matched when the query's predicate is written the same way, so an index can
be unused because the query drifted rather than because the feature is gone
(migration 115 and the `last_err` predicate in `auto_resume.rs` are pinned to
each other by a unit test for exactly this reason).

## The `stats_reset` caveat

Every counter above is cumulative since the last reset, and **all of them reset
on a server restart, a crash, or a major-version upgrade** — the PG 18 upgrade
on 2026-09-12 zeroed them. Always read the window before reading the numbers:

```sql
SELECT stats_reset, now() - stats_reset AS window FROM pg_stat_statements_info;
SELECT stats_reset, now() - stats_reset AS window FROM pg_stat_database
WHERE datname = current_database();
```

`pg_stat_user_indexes` has no `stats_reset` of its own; it follows the
`pg_stat_database` one. So "85 scans ever" means 85 scans *in this window*, and
an index that looks dead an hour after a restart may simply be waiting for the
first search of the day. Judge a rarely-used index over days, not minutes.

`pg_stat_statements` also has a fixed `pg_stat_statements.max` (5000 by
default): once it is full, the least-executed entries are evicted, so a rare
expensive statement can vanish entirely. `SELECT count(*) FROM
pg_stat_statements;` at the limit means the tail is not trustworthy.
