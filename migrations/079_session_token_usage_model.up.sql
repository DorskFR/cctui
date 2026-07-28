-- Per-row model attribution for token usage.
--
-- Dollar budgets price each row against the account's model catalog, and the
-- catalog is per model — a session-level model is not enough (a session may
-- switch models mid-run, and subagents use another). NULL for rows recorded
-- before this column existed: they are unpriceable, not free, so USD windows
-- skip them rather than charging a guessed rate.
ALTER TABLE session_token_usage ADD COLUMN IF NOT EXISTS model text;

CREATE INDEX IF NOT EXISTS session_token_usage_created_idx
    ON session_token_usage (created_at);
