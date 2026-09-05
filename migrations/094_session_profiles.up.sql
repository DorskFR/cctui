-- Per-user spawn profiles: a named harness/account/model/effort/permission
-- kit the spawn panel selects in one click. The account pick is one of: an
-- account, a pool, no account (the machine's own login), or none of them =
-- Auto. NULL permission_mode = "Default" (the account default, else the
-- harness's own).
CREATE TABLE IF NOT EXISTS session_profiles (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    harness         TEXT NOT NULL DEFAULT 'claude-code',
    account_id      UUID REFERENCES accounts(id) ON DELETE SET NULL,
    pool_id         UUID REFERENCES account_pools(id) ON DELETE SET NULL,
    no_account      BOOLEAN NOT NULL DEFAULT FALSE,
    model_alias     TEXT,
    effort          TEXT,
    permission_mode TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name),
    CHECK (num_nonnulls(account_id, pool_id) + no_account::int <= 1)
);
