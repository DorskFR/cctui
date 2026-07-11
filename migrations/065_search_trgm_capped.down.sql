CREATE INDEX IF NOT EXISTS idx_stream_events_search_trgm
    ON stream_events USING gin (search_text gin_trgm_ops);

DROP INDEX IF EXISTS idx_stream_events_search_trgm_capped;
