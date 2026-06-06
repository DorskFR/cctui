-- CCT-232: OAuth account vault + gateway passthrough.
--
-- Users register named OAuth accounts (e.g. `personal`, `enterprise`) for
-- Claude Code / Codex. The server stores the OAuth tokens encrypted at rest
-- (crate::crypto, same vault key as api_keys/dispatchers), never returns them
-- over the API, and never lets them reach worker pods. Instead a thin gateway
-- (routes/gateway.rs) swaps a session-scoped cctui token for the account's
-- current access token on each upstream request, refreshing under a per-account
-- mutex when near expiry.

CREATE TABLE IF NOT EXISTS oauth_accounts (
    id                       UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id                  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name                     TEXT NOT NULL,
    provider                 TEXT NOT NULL,            -- 'anthropic' | 'openai'
    encrypted_access_token   TEXT,                     -- nullable: refresh-only until first refresh
    encrypted_refresh_token  TEXT NOT NULL,
    expires_at               TIMESTAMPTZ,              -- access-token expiry (NULL = unknown → refresh on use)
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at             TIMESTAMPTZ,
    request_count            BIGINT NOT NULL DEFAULT 0,
    bytes_transferred        BIGINT NOT NULL DEFAULT 0
);

-- A user cannot have two accounts with the same (name, provider).
CREATE UNIQUE INDEX IF NOT EXISTS oauth_accounts_user_name_provider
    ON oauth_accounts (user_id, name, provider);

-- Session-scoped gateway tokens (CCT-232): minted at spawn/dispatch, map a
-- random bearer the worker carries to the (session, account) it should use.
-- Raw OAuth tokens never enter the worker; only this opaque token does. Hashed
-- like every other cctui credential so a DB read can't replay it.
CREATE TABLE IF NOT EXISTS session_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    token_hash  TEXT NOT NULL UNIQUE,
    session_id  TEXT NOT NULL,
    account_id  UUID NOT NULL REFERENCES oauth_accounts(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS session_tokens_session ON session_tokens (session_id);
