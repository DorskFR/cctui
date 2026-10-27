-- Last workflow-guard step each live session's pod reported, so any replica
-- can answer it. Removed when the session ends, is killed or archived.
CREATE TABLE IF NOT EXISTS guard_steps (
    session_id  TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    state       JSONB NOT NULL,
    reported_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
