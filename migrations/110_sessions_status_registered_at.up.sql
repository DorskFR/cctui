-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_sessions_status_registered_at
    ON sessions (status, registered_at DESC);
