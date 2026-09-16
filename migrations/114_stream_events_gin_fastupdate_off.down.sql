ALTER TABLE stream_events RESET (
    autovacuum_analyze_scale_factor,
    autovacuum_vacuum_scale_factor
);

ALTER INDEX idx_stream_events_session_search_trgm SET (fastupdate = on);
