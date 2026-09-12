-- no-transaction
--
-- `ORDER BY s.registered_at DESC LIMIT n` is the shape of every search and of
-- the archive browse. Without an index the planner sorts all sessions before
-- applying the limit, which forces the free-text probe to run for every session
-- instead of stopping once the page is full.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_sessions_registered_at_desc
    ON sessions (registered_at DESC);
