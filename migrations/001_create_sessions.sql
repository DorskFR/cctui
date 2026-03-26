CREATE TABLE IF NOT EXISTS sessions (
    id              UUID PRIMARY KEY,
    parent_id       UUID REFERENCES sessions(id) ON DELETE SET NULL,
    account_id      UUID,
    machine_id      TEXT NOT NULL,
    working_dir     TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'registering',
    registered_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_heartbeat  TIMESTAMPTZ NOT NULL DEFAULT now(),
    metadata        JSONB NOT NULL DEFAULT '{}'
);

CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_id);
CREATE INDEX IF NOT EXISTS idx_sessions_status ON sessions(status);
CREATE INDEX IF NOT EXISTS idx_sessions_machine ON sessions(machine_id);
