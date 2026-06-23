-- CCT-476: make a session's gateway token STABLE for the worker's whole life.
--
-- Previously every resume re-minted a brand-new session_tokens row (new hash),
-- which (a) bloated the table and (b) left live workers presenting a token the
-- gateway sometimes could not resolve. We now mint ONE token per session and
-- reuse it across resumes, repointing its account_id on account switch instead
-- of minting anew. Reuse requires re-supplying the original opaque token string,
-- so we persist it obfuscated (same scheme as oauth_accounts credentials); only
-- the hash was stored before, which is one-way.
ALTER TABLE session_tokens ADD COLUMN IF NOT EXISTS encrypted_token TEXT;

-- Reuse looks up the live token for a session; index the lookup.
CREATE INDEX IF NOT EXISTS session_tokens_session_live
    ON session_tokens (session_id)
    WHERE revoked_at IS NULL;
