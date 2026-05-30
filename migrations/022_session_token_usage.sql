-- Per-message token usage (CCT-94).
--
-- Claude's transcript carries a `usage` block on each assistant message
-- with input/output/cache_read/cache_creation token counts. We persist
-- one row per assistant message keyed by `(session_id, message_id)`
-- so daemon re-tail (which replays already-forwarded lines) is
-- idempotent by construction. Aggregation for the API happens at
-- read time via SUM().

CREATE TABLE IF NOT EXISTS session_token_usage (
    session_id            text         NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    message_id            text         NOT NULL,
    input_tokens          bigint       NOT NULL DEFAULT 0,
    output_tokens         bigint       NOT NULL DEFAULT 0,
    cache_read_tokens     bigint       NOT NULL DEFAULT 0,
    cache_creation_tokens bigint       NOT NULL DEFAULT 0,
    created_at            timestamptz  NOT NULL DEFAULT now(),
    PRIMARY KEY (session_id, message_id)
);

CREATE INDEX IF NOT EXISTS session_token_usage_session_idx
    ON session_token_usage (session_id);
