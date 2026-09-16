-- no-transaction
--
-- Same definition as migration 065, built CONCURRENTLY because on a populated
-- database a plain CREATE INDEX holds ACCESS EXCLUSIVE for tens of minutes on
-- the startup path.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_stream_events_search_trgm_capped
    ON stream_events USING gin (left(search_text, 8192) gin_trgm_ops);
