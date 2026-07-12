-- CCT-580: per-(user, session) read high-water mark. A single `last_seen_at`
-- per user per session is enough to derive an unread-message count (assistant
-- `message` events newer than the mark) for the sessions list, without a
-- per-message read model. Server-side + per-user because the same account is
-- used from multiple devices, so localStorage would desync.
CREATE TABLE session_reads (
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, session_id)
);
CREATE INDEX session_reads_user_idx ON session_reads (user_id);
