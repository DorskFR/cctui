-- The user a gateway session token was minted FOR, as distinct from the user who
-- owns the account it resolves to. On a shared account the two differ, and every
-- trust decision downstream — which settings/env keys cross into the worker,
-- whether a re-mint is still authorised, which tokens a share revoke must cut —
-- needs the grantee's identity, not the owner's.
ALTER TABLE session_tokens
    ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id) ON DELETE CASCADE;

-- Backfill: the session's own owner where it is known, else the account owner
-- (the only possibility before sharing existed).
UPDATE session_tokens st
   SET user_id = COALESCE(
           (SELECT s.user_id FROM sessions s WHERE s.id = st.session_id),
           (SELECT ap.user_id FROM account_providers ap WHERE ap.id = st.account_id))
 WHERE st.user_id IS NULL;

-- Revoke-by-grantee on a share revoke, and the owner check on the daemon
-- gateway-env pull, both look tokens up by user.
CREATE INDEX IF NOT EXISTS session_tokens_user_live
    ON session_tokens (user_id)
    WHERE revoked_at IS NULL;
