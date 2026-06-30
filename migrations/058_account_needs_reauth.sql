-- CCT-512: account credential health. When the gateway sees the upstream provider
-- reject an account's OAuth credentials (a persistent provider-oauth 401 / refresh
-- failure), it flags the account here; a successful upstream call clears it. The
-- accounts UI surfaces the flag as a "reauthenticate" badge + button.
ALTER TABLE oauth_accounts
    ADD COLUMN needs_reauth        BOOLEAN     NOT NULL DEFAULT false,
    ADD COLUMN last_auth_error     TEXT,
    ADD COLUMN last_auth_error_at  TIMESTAMPTZ;
