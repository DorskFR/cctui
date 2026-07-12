-- CCT-566: agent-posted images. The daemon detects an `![alt](/abs/path.png)`
-- marker in an assistant message, uploads the file here, and rewrites the marker
-- to `![alt](cctui-img://<id>)` so the bytes ride the existing text payload while
-- the picture is served from this store. Postgres `bytea` (not pod-local FS,
-- which is ephemeral in prod; the GET endpoint keeps an S3 swap open later).
-- `sha256` dedups re-posts of the same bytes within a session. Rows cascade away
-- with the session (GC on purge, CCT-566 #5).
CREATE TABLE session_images (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id  TEXT NOT NULL REFERENCES sessions (id) ON DELETE CASCADE,
    sha256      TEXT NOT NULL,
    media_type  TEXT NOT NULL,
    byte_len    INTEGER NOT NULL,
    bytes       BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (session_id, sha256)
);

CREATE INDEX session_images_session_idx ON session_images (session_id);
