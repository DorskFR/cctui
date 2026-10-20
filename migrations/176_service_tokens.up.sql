-- Owner-minted gateway credentials for non-agent services: no session row,
-- no expiry, revocable from the account page. Existing rows are session tokens.
ALTER TABLE session_tokens
    ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'session',
    ADD COLUMN IF NOT EXISTS label TEXT;

CREATE INDEX IF NOT EXISTS session_tokens_service_by_account
    ON session_tokens (account_id, created_at)
    WHERE kind = 'service';
