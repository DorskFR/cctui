-- Per-user spawn profiles: a named harness/account/model/effort/permission
-- kit the spawn panel selects in one click. NULL permission_mode = "Default"
-- (the account default, else the harness's own).
CREATE TABLE IF NOT EXISTS session_profiles (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    harness         TEXT NOT NULL DEFAULT 'claude-code',
    account_id      UUID REFERENCES accounts(id) ON DELETE SET NULL,
    model_alias     TEXT,
    effort          TEXT,
    permission_mode TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);
