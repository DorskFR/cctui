# Transcript search index

Search reads `search_chunks`: the text of each stream event as the webui shows
it (markdown markers, link URLs, code fences and ANSI removed; NFKC; smart quotes
straightened; whitespace collapsed), split into ~4 KB chunks that overlap by
512 bytes. Base64 image/document blocks and other 200+ character base64/hex runs
are not indexed; at most 64 KiB of text is indexed per event.

A quoted query is an exact phrase over that text; unquoted words must all
appear. Queries are folded the same way, so a phrase copied from the rendered
conversation matches.

## Backfill

Daemon events are chunked on ingest. Every other row, including history from
before the table existed, is reached by a background walk over
`stream_events` in id order: small batches, one short transaction each, cursor
in `search_index_state` (`name = 'backfill'`). It is idempotent and resumes
where it stopped after a restart. One replica walks at a time (advisory lock).

| Variable | Default | Effect |
| --- | --- | --- |
| `CCTUI_SEARCH_BACKFILL` | on | `0`/`false`/`off`/`no` pauses the walk; the cursor stays put |
| `CCTUI_SEARCH_BACKFILL_BATCH` | `50` | events per batch |
| `CCTUI_SEARCH_BACKFILL_PAUSE_MS` | `250` | sleep between full batches |

Progress:

```sql
SELECT s.last_event_id, (SELECT max(id) FROM stream_events) AS max_id, s.updated_at
FROM search_index_state s WHERE s.name = 'backfill';

SELECT pg_size_pretty(pg_total_relation_size('search_chunks'));
```

## Unchunked history

`search_index_state` row `live_from` holds the first event id chunked on
ingest (written once; migration 187 seeds it on installs that already chunk).
Events with an id past the backfill cursor and below `live_from` have no chunks,
so search also matches them the way it did before chunks existed: the unfolded
query against `left(search_text, 8192)`, through
`idx_stream_events_session_search_trgm`. That matching skips the 8 KB tail and
the folding. The fallback goes away once the cursor reaches `live_from`.

```sql
SELECT name, last_event_id FROM search_index_state WHERE name IN ('backfill', 'live_from');
```

Rows written after `live_from` by paths other than daemon ingest are still only
found once the walk reaches them.
