-- Session-scoped trigram index + session ordering indexes for CCT-1006.
--
-- The free-text predicate is a per-session `EXISTS … WHERE e.session_id = s.id
-- AND left(e.search_text, 8192) ILIKE $p`. Served by the unscoped trgm index
-- (migration 065) the planner could only answer it by materialising every
-- matching event in the table and rechecking each one — 300k heap rechecks,
-- ~13 s, for a question whose answer is at most one row per session. Adding
-- session_id as a leading GIN key (btree_gin) lets the intersection happen in
-- the index, so a probe touches only the events of the session being tested.
--
-- The 8192 literal must match SEARCH_TEXT_CAP in routes/sessions.rs, exactly as
-- for migration 065 — the query's ILIKE runs against the same left() expression
-- so this index serves it only if the two agree.
CREATE EXTENSION IF NOT EXISTS btree_gin;

CREATE INDEX IF NOT EXISTS idx_stream_events_session_search_trgm
    ON stream_events USING gin (session_id, left(search_text, 8192) gin_trgm_ops);

-- `ORDER BY s.registered_at DESC LIMIT n` is the shape of every search and of
-- the archive browse. Without an index the planner sorts all sessions before
-- applying the limit, which forces the free-text probe to run for every session
-- instead of stopping once the page is full.
CREATE INDEX IF NOT EXISTS idx_sessions_registered_at_desc
    ON sessions (registered_at DESC);

CREATE INDEX IF NOT EXISTS idx_sessions_status_registered_at
    ON sessions (status, registered_at DESC);
