-- Auto-resume after a mid-stream connection loss (see crates/cctui-server/src/auto_resume.rs).
-- One row per session: which API-error message we are recovering from, how many
-- "continue" nudges were sent and when the next one is due.
CREATE TABLE IF NOT EXISTS session_auto_resume (
    session_id      TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    error_event_id  BIGINT NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    state           TEXT NOT NULL DEFAULT 'pending',
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error      TEXT,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
