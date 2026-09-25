-- no-transaction
--
-- Newest-turns-per-session lookups (the sessions list's last two turns, the
-- conversation page's usage window) walk this index backwards per session.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_session_token_usage_session_created
    ON session_token_usage (session_id, created_at DESC);
