-- CCT-410: uniform RBAC. Everyone is a user; admin is a user with the `admin`
-- scope (no more user_id=NULL ghost). A unified `auth_keys` table supersedes
-- users.key_hash / user_tokens / machines.key_hash / dispatchers.key_hash, and
-- two normalized ACL tables carry scopes:
--   * user_acls = the user's *ceiling* (what they may delegate)
--   * key_acls  = a key's *granted* subset (least privilege)
-- Effective authority is intersected per-request: key_acls ∩ user_acls.
--
-- This migration is SAFE + TRANSPARENT: zero capability regression. Every
-- existing credential keeps working unchanged because (a) the legacy tables are
-- preserved and the auth path dual-reads them, and (b) every legacy key is also
-- backfilled into auth_keys with a grant equal to its owner's full ceiling.

-- ---------------------------------------------------------------------------
-- 1. Unified key table.
-- ---------------------------------------------------------------------------
-- `kind` records what the legacy row was so the validate() machine-path
-- (archive/skills) keeps working: a `machine` key carries `machine_id`.
-- `dispatcher_id` links a dispatcher enrollment key back to its row.
CREATE TABLE IF NOT EXISTS auth_keys (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key_hash      TEXT NOT NULL UNIQUE,
    key_preview   TEXT,
    label         TEXT,
    kind          TEXT NOT NULL DEFAULT 'user',   -- 'user' | 'machine' | 'dispatcher' | 'admin'
    machine_id    UUID REFERENCES machines(id) ON DELETE CASCADE,
    dispatcher_id UUID REFERENCES dispatchers(id) ON DELETE CASCADE,
    expires_at    TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_auth_keys_user ON auth_keys(user_id);
CREATE INDEX IF NOT EXISTS idx_auth_keys_machine ON auth_keys(machine_id) WHERE machine_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_auth_keys_dispatcher ON auth_keys(dispatcher_id) WHERE dispatcher_id IS NOT NULL;

-- ---------------------------------------------------------------------------
-- 2. Normalized ACL tables — one row per (subject, scope). Real FK + CASCADE.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS user_acls (
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope      TEXT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, scope)
);

CREATE TABLE IF NOT EXISTS key_acls (
    key_id     UUID NOT NULL REFERENCES auth_keys(id) ON DELETE CASCADE,
    scope      TEXT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (key_id, scope)
);

-- ---------------------------------------------------------------------------
-- 3. Seed an admin USER and break-glass admin keys from CCTUI_ADMIN_TOKENS.
--    The env tokens (their sha256 hashes) are seeded by the server at startup
--    (it has the plaintext); this migration only guarantees the schema exists.
--    See crate::auth::AuthConfig::seed_admin.
-- ---------------------------------------------------------------------------

-- ---------------------------------------------------------------------------
-- 4. Backfill every EXISTING user's ceiling = exactly what they can do today:
--    {read, enroll} always; {dispatch} only where can_dispatch is true. Admin
--    is NOT granted to ordinary users (the seeded admin user is separate).
-- ---------------------------------------------------------------------------
INSERT INTO user_acls (user_id, scope)
    SELECT id, 'read' FROM users
    ON CONFLICT DO NOTHING;
INSERT INTO user_acls (user_id, scope)
    SELECT id, 'enroll' FROM users
    ON CONFLICT DO NOTHING;
INSERT INTO user_acls (user_id, scope)
    SELECT id, 'dispatch' FROM users WHERE can_dispatch
    ON CONFLICT DO NOTHING;

-- ---------------------------------------------------------------------------
-- 5. Backfill every existing KEY into auth_keys, with a key_acls grant equal to
--    the owner's full ceiling, so each existing token does exactly what it does
--    today (no narrowing). Idempotent (re-run safe) via ON CONFLICT on the
--    unique key_hash, so a fresh random id per insert is fine.
-- ---------------------------------------------------------------------------

-- 5a. Legacy users.key_hash (original single-token-per-user model, migration 008).
INSERT INTO auth_keys (id, user_id, key_hash, key_preview, label, kind, created_at)
    SELECT
        gen_random_uuid(),
        u.id, u.key_hash, NULL, 'primary (migrated)', 'user', u.created_at
    FROM users u
    WHERE u.key_hash IS NOT NULL
    ON CONFLICT (key_hash) DO NOTHING;

-- 5b. user_tokens (many-tokens model, migration 014).
INSERT INTO auth_keys (id, user_id, key_hash, key_preview, label, kind, expires_at, revoked_at, created_at)
    SELECT
        gen_random_uuid(),
        t.user_id, t.token_hash, t.token_preview, t.label, 'user', t.expires_at, t.revoked_at, t.created_at
    FROM user_tokens t
    ON CONFLICT (key_hash) DO NOTHING;

-- 5c. machines.key_hash.
INSERT INTO auth_keys (id, user_id, key_hash, key_preview, label, kind, machine_id, revoked_at, created_at)
    SELECT
        gen_random_uuid(),
        m.user_id, m.key_hash, m.key_preview, m.name, 'machine', m.id, m.revoked_at, m.first_seen_at
    FROM machines m
    WHERE m.key_hash IS NOT NULL AND m.deleted_at IS NULL
    ON CONFLICT (key_hash) DO NOTHING;

-- 5d. dispatchers.key_hash (enrolled dispatchers, migration 040).
INSERT INTO auth_keys (id, user_id, key_hash, key_preview, label, kind, dispatcher_id, revoked_at, created_at)
    SELECT
        gen_random_uuid(),
        d.user_id, d.key_hash, d.key_preview, d.name, 'dispatcher', d.id, d.revoked_at, d.created_at
    FROM dispatchers d
    WHERE d.key_hash IS NOT NULL AND d.deleted_at IS NULL
    ON CONFLICT (key_hash) DO NOTHING;

-- 5e. Grant each migrated key the OWNER'S FULL CEILING (transparency: existing
--     tokens keep doing exactly what they do today). Re-run safe.
INSERT INTO key_acls (key_id, scope)
    SELECT k.id, ua.scope
    FROM auth_keys k
    JOIN user_acls ua ON ua.user_id = k.user_id
    ON CONFLICT DO NOTHING;
