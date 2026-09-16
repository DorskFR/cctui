-- With fastupdate on, the remaining GIN index buffers new entries in a pending
-- list and the backend that happens to cross gin_pending_list_limit pays the
-- whole flush synchronously — that is the 10 s `INSERT INTO stream_events` on
-- the daemon ingest path. Off, each insert pays its own few milliseconds.
ALTER INDEX idx_stream_events_session_search_trgm SET (fastupdate = off);

-- stream_events had never been analyzed: the table-level defaults (0.1/0.2)
-- need 135k row changes before autovacuum looks at it.
ALTER TABLE stream_events SET (
    autovacuum_analyze_scale_factor = 0.02,
    autovacuum_vacuum_scale_factor = 0.05
);
