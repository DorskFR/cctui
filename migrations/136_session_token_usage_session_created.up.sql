-- no-transaction
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_session_token_usage_session_created
    ON session_token_usage (session_id, created_at)
    INCLUDE (model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens);
