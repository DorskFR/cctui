-- Per-user token issuance. Replaces the legacy `users.key_hash` single-token
-- model (kept for back-compat in 008) with a many-tokens-per-user table so
-- users can mint additional tokens (labelled, revocable, optionally expiring).
-- The original `users.key_hash` column stays populated for back-compat; the
-- auth path checks both surfaces.

CREATE TABLE IF NOT EXISTS user_tokens (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    token_hash  TEXT NOT NULL UNIQUE,
    label       TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at  TIMESTAMPTZ,
    revoked_at  TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_user_tokens_user_active
    ON user_tokens(user_id)
    WHERE revoked_at IS NULL;
