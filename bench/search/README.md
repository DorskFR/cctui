# Session-search performance bench (CCT-1006)

Reproducible harness for the before/after numbers on `GET /sessions/search`.

## Dataset

No realistic dataset exists locally (the dev database is empty, the test database
has 181 sessions), so the bench builds one:

- 3000 sessions (90% archived), 12 machines, 20 labels
- 600 000 `stream_events`, 200 per session
- `search_text` averaging 3650 bytes — large enough to be TOASTed, which is what
  makes the bitmap recheck expensive
- marker terms of deliberately different selectivity: `error` 300 000 events
  (50%), `quokka` 6185 (1%), `zygomorphic` 120 (0.02%), `ok` sub-trigram
- 576 MB table, 187 MB trgm index

```sh
createdb cctui_bench
DATABASE_URL=postgres://…/cctui_bench sqlx migrate run --source migrations
psql -d cctui_bench -f bench/search/seed.sql
```

`queries-before.sql` is the SQL the handler emitted before CCT-1006,
`queries-after.sql` the SQL it emits now. Run each against the same database —
`queries-before.sql` needs migration 106 rolled back to reproduce the old plans.

## Results

`EXPLAIN (ANALYZE, BUFFERS)` execution time, limit 100, warm cache:

| shape | before | after |
|---|---:|---:|
| A single common term (`error`, 50% of events) | 12 957 ms | **353 ms** |
| B single rare term (`zygomorphic`, 0.02%) | 400 ms | **38 ms** |
| C two terms (`quokka handler`) | 24 129 ms | **1 248 ms** |
| D negated term (`NOT zygomorphic`) | 281 ms | 281 ms |
| E sub-trigram term (`ok`) | 25 705 ms | **7 ms** |
| F fielded filter (`tag:tag7`) | 2 ms | 2 ms |
| G empty-q archive browse | 2 ms | 2 ms |
| H archive browse at offset 2000 | 2 ms | 2 ms |
| I snippet pass, 100 ids, common term | 654 ms | 798 ms |
| J snippet pass, 500 ids, common term | 3 530 ms | 5 004 ms |

## Attribution

The bitmap *index* scan for `error` costs 28 ms; the *heap recheck* over the
300 000 matching rows costs ~13 s — 43 µs per row, spent detoasting a 3.6 KB
`search_text` and running a Unicode-folding `ILIKE` over it. Per-row cost splits
as ~6.8 µs detoast, ~11 µs for case-sensitive `LIKE`, ~43 µs for `ILIKE`.

So the whole-table recheck dominated, not the index and not the enrichment
fan-out. The fix removes the recheck volume rather than the recheck cost: probe
one session at a time and stop at the first hit.

## Candidates rejected, with the measurement that rules them out

- **Keyset pagination for archive browse.** Deep OFFSET is not a problem:
  offset 2000 measures 1.6 ms, same as offset 0 (G vs H). The
  `sessions(status, registered_at)` index the ticket asks for is added anyway
  because it is what lets the search path stop at the page limit.
- **Cutting the enrichment fan-out.** Nothing in the measurements implicates it:
  the response time was dominated by a single query whose recheck ran for 13 s.
  It also cannot be trimmed without changing what the endpoint returns, which
  this change is required not to do.
- **A tsvector/FTS column.** Would not preserve results — `ILIKE '%err%'` matches
  inside words and `to_tsquery` does not. A correctness change, not a perf one.
- **Rejecting sub-trigram terms early.** Unnecessary once the probe is
  session-scoped: `ok` went from 25 705 ms to 7 ms without special-casing, so the
  short-term path no longer needs its own treatment.
- **Pre-lowercasing `search_text` to use `LIKE` instead of `ILIKE`.** Worth ~4x
  per row, but it duplicates up to 8 KiB per event (~2 GB on this dataset) and
  the row count, not the per-row cost, was the problem. Kept in reserve.

## Known remaining gap

Two-term queries still cost 1.2 s here (C). Both are worst cases of the
synthetic data: `handler` occurs in *every one* of the 600 000 events, so its
trigram posting lists are maximal and the per-session GIN intersection costs
~3.7 ms. Real transcripts do not have a word in 100% of events.

The snippet pass (I/J) is the one shape that did not improve. Its structure is
strictly better — one row per session instead of every matching event — but the
planner now reaches for the session-scoped trgm index where an ordered scan of
`idx_stream_events_session` with early exit is faster for common terms (55 ms at
500 ids when forced). The reverse holds for rare terms (11 ms via trgm, 3978 ms
forced). Postgres cannot tell the two apart: it estimates every `%pattern%` at
960 rows regardless of the term, and raising the statistics target does not move
that estimate. Picking the right plan needs a term-frequency signal the database
does not have.
