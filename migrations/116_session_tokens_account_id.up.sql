-- no-transaction
--
-- PROVIDER_SELECT aggregates token usage per provider through a correlated
-- LATERAL over session_tokens; without this index each provider costs a seq
-- scan of the table. session_id is included so the join to session_token_usage
-- is satisfied from the index alone.
CREATE INDEX CONCURRENTLY IF NOT EXISTS idx_session_tokens_account_id
    ON session_tokens (account_id, session_id);
