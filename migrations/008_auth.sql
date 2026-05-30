CREATE TABLE IF NOT EXISTS users (
    id          UUID PRIMARY KEY,
    name        TEXT NOT NULL,
    key_hash    TEXT NOT NULL UNIQUE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at  TIMESTAMPTZ
);

CREATE TABLE IF NOT EXISTS machines (
    id             UUID PRIMARY KEY,
    user_id        UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name           TEXT NOT NULL,
    key_hash       TEXT NOT NULL UNIQUE,
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at     TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_machines_user ON machines(user_id);

ALTER TABLE sessions ADD COLUMN IF NOT EXISTS user_id      UUID REFERENCES users(id);
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS machine_uuid UUID REFERENCES machines(id);

CREATE INDEX IF NOT EXISTS idx_sessions_user    ON sessions(user_id);
CREATE INDEX IF NOT EXISTS idx_sessions_machine_uuid ON sessions(machine_uuid);
