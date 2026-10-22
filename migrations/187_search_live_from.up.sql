-- Search matches ids between the backfill cursor and the first event chunked on
-- ingest through stream_events.search_text, so that first id must be exact.
-- An install already chunking on ingest takes its oldest chunked event created
-- since migration 186 (privacy reindex can chunk older ones); any other install
-- records it on its first ingest.
INSERT INTO search_index_state (name, last_event_id)
SELECT 'live_from', min(c.event_id)
FROM search_chunks c
JOIN stream_events e ON e.id = c.event_id
WHERE c.event_id > COALESCE(
        (SELECT last_event_id FROM search_index_state WHERE name = 'backfill'), 0)
  AND e.created_at >= (SELECT installed_on FROM _sqlx_migrations WHERE version = 186)
HAVING min(c.event_id) IS NOT NULL
ON CONFLICT (name) DO NOTHING;
