-- Command queue — stub. v0 reserves the table so the daemon→adapter write
-- path (SendMessage, Kill, Spawn) can be persisted and replayed on
-- reconnect. The WS path delivers commands live; this table is for
-- audit + post-v0 offline / poll fallback.

CREATE TABLE IF NOT EXISTS commands (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    payload    JSONB NOT NULL DEFAULT '{}'::jsonb,
    status     TEXT NOT NULL DEFAULT 'pending',
    issued_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_commands_session_pending
    ON commands(session_id)
    WHERE status = 'pending';
