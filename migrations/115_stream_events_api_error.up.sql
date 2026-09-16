-- no-transaction
--
-- The auto-resume sweep asks every 20 s for the newest `API Error:` assistant
-- message per session. 1k of 1.35M rows match, but without a partial index the
-- planner skip-scans the whole `(session_id, created_at)` index: ~21k buffers
-- per call, the top total-time statement in pg_stat_statements.
--
-- The predicate must stay character-identical to the `last_err` CTE in
-- auto_resume.rs (STUCK_SELECT) or the planner will not match it.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_stream_events_api_error
    ON stream_events (created_at DESC)
    WHERE event_type = 'message'
      AND payload->>'role' = 'assistant'
      AND payload->>'text' LIKE 'API Error:%';
