-- no-transaction
-- Claude prepends its login hint to gateway 401s. Keep the existing API Error
-- index, and use this complementary predicate via UNION ALL in auto_resume.rs.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_stream_events_login_error
    ON stream_events (created_at DESC)
    WHERE event_type = 'message'
      AND payload->>'role' = 'assistant'
      AND payload->>'text' LIKE 'Please run /login%';
