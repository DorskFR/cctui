-- CCT-235: user-defined named dispatchers. Per-account dispatcher definitions
-- (type + params) that can be referenced by name in POST /sessions/dispatch,
-- alongside the global env-configured registry (which remains as fallback).
--
-- `config` is a type-specific JSONB blob. Secret fields inside it (http bearer
-- token, etc.) are encrypted at the application layer (crate::crypto) before
-- being stored, and redacted out of every API/list/notification response.
CREATE TABLE IF NOT EXISTS dispatchers (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,            -- 'http' | 'kubernetes'
    config     JSONB NOT NULL,           -- type-specific params; secrets encrypted in-place
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    deleted_at TIMESTAMPTZ
);

-- A user cannot have two live dispatchers with the same name; soft-deleted rows
-- don't count so a name can be reused after deletion.
CREATE UNIQUE INDEX IF NOT EXISTS dispatchers_user_name_live
    ON dispatchers (user_id, name)
    WHERE deleted_at IS NULL;
