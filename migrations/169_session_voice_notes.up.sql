CREATE TABLE session_voice_notes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id  TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    media_type  TEXT NOT NULL,
    byte_len    INTEGER NOT NULL,
    duration_ms INTEGER,
    text        TEXT NOT NULL,
    bytes       BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX session_voice_notes_session_idx ON session_voice_notes (session_id);
