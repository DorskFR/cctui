-- Files the user uploads mid-chat (paste-N.txt masks, screenshots, docs). The
-- bytes live in the content-addressed daemon_blobs store; this row ties a blob
-- to the session it was sent in so the conversation can show it again. Rows
-- cascade away with the session. message_id is filled once the transcript
-- echoes an id for the user turn; until then the webui links by name + time.
CREATE TABLE session_attachments (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id   TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    message_id   TEXT,
    name         TEXT NOT NULL,
    hash         TEXT NOT NULL,
    size         BIGINT NOT NULL,
    content_type TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX session_attachments_session_idx ON session_attachments (session_id, created_at);
