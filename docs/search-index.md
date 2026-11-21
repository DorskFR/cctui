# Transcript search index

Search reads `search_chunks`: the text of each stream event as the webui shows
it (markdown markers, link URLs, code fences and ANSI removed; NFKC; smart quotes
straightened; whitespace collapsed), split into ~4 KB chunks that overlap by
512 bytes. Base64 image/document blocks and other 200+ character base64/hex runs
are not indexed; at most 64 KiB of text is indexed per event.

A quoted query is an exact phrase over that text; unquoted words must all
appear. Queries are folded the same way, so a phrase copied from the rendered
conversation matches.

> **Upgrading from 0.32.0-beta.14 or older.** Search no longer falls back to the
> `stream_events.search_text` column. Right after the upgrade, search finds only
> events that already have chunks; older history becomes searchable as the
> backfill below walks it (on by default). This is the trade for faster search
> and smaller storage. A server older than this release refuses to start on a
> database this release migrated.

## Backfill

Daemon events are chunked on ingest. Every other row, including history from
before the table existed, is reached by a background walk over
`stream_events` in id order: small batches, one short transaction each, cursor
in `search_index_state` (`name = 'backfill'`). The walk skips events that
already have chunks, so rows chunked on ingest are not chunked again. It is
idempotent and resumes where it stopped after a restart. One replica walks at a
time (advisory lock). An event is found by search only once it has chunks.

| Variable | Default | Effect |
| --- | --- | --- |
| `CCTUI_SEARCH_BACKFILL` | on | `0`/`false`/`off`/`no` pauses the walk; the cursor stays put |
| `CCTUI_SEARCH_BACKFILL_BATCH` | `50` | events per batch |
| `CCTUI_SEARCH_BACKFILL_PAUSE_MS` | `250` | sleep between full batches |
| `CCTUI_SEARCH_BACKFILL_BUDGET_MB` | unlimited | chunk body bytes this process writes before the walk stops; restart to continue from the cursor |

Every 100 batches the walk logs `search backfill progress` with its cursor,
`max_id`, and the chunks and bytes it wrote. It logs
`search backfill budget spent` when it stops on the budget.

Progress:

```sql
SELECT s.last_event_id, (SELECT max(id) FROM stream_events) AS max_id, s.updated_at
FROM search_index_state s WHERE s.name = 'backfill';

SELECT pg_size_pretty(pg_total_relation_size('search_chunks'));
```
