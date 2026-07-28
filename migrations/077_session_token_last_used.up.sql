-- Observed-identity signal (CCT-575): stamp when a session's gateway token was
-- last presented at the gateway, so the UI can flag an account-bound session
-- whose worker's traffic never actually reached the gateway (silently riding
-- ambient creds instead). NULL = never observed.
ALTER TABLE session_tokens ADD COLUMN IF NOT EXISTS last_used_at TIMESTAMPTZ;
