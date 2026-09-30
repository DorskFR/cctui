-- Reusable per-user context attached to a session at spawn: durable memory
-- notes and prompt templates. Skills are NOT here — a skill bundle is a
-- plugin, delivered per session by the plugin platform (see
-- docs/adr/0002-session-context.md).
--
-- `scope` decides when an item is auto-resolved for a spawn, and `scope_ref`
-- carries the target it needs: a machine id, a working-dir prefix, a label id.
-- Scopes union rather than override: several memories legitimately apply to
-- one session.
CREATE TABLE IF NOT EXISTS context_items (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    name       TEXT NOT NULL,
    title      TEXT NOT NULL,
    body       TEXT NOT NULL DEFAULT '',
    scope      TEXT NOT NULL DEFAULT 'user',
    scope_ref  TEXT,
    tags       TEXT[] NOT NULL DEFAULT '{}',
    enabled    BOOLEAN NOT NULL DEFAULT TRUE,
    version    INTEGER NOT NULL DEFAULT 1,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, kind, name),
    CHECK (kind IN ('memory', 'prompt')),
    CHECK (scope IN ('user', 'machine', 'path', 'label')),
    -- 'user' needs no target; every other scope is meaningless without one.
    CHECK ((scope = 'user') = (scope_ref IS NULL))
);

-- The auto-resolution read: everything enabled for one owner, by kind.
CREATE INDEX IF NOT EXISTS context_items_owner_idx
    ON context_items (user_id, kind) WHERE enabled;

-- The set a spawn actually resolved, keyed by the launch key the daemon later
-- pulls its gateway env with — the same key `spawn_capabilities` and the
-- label/follow-up intents use. Written at spawn, read once at launch.
CREATE TABLE IF NOT EXISTS session_context (
    spawn_key  TEXT PRIMARY KEY,
    item_ids   UUID[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A profile's context set, applied SERVER-side at spawn so a webui spawn, a
-- `CctuiAgent` child and a dispatch launched from the same profile get the
-- same kit. Profiles are otherwise applied by the browser overwriting the
-- spawn form, which children never see.
ALTER TABLE session_profiles
    ADD COLUMN IF NOT EXISTS context_items UUID[] NOT NULL DEFAULT '{}';
