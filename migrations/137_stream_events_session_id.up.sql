-- no-transaction
--
-- Conversation pages read `WHERE session_id = $1 AND id < $2 ORDER BY id DESC
-- LIMIT n`; this makes each page an index range scan of n entries instead of
-- reading and sorting the whole session.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_stream_events_session_id
    ON stream_events (session_id, id);
