CREATE TABLE IF NOT EXISTS archive_index (
    id             BIGSERIAL PRIMARY KEY,
    machine_id     UUID NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    project_dir    TEXT NOT NULL,
    session_id     TEXT NOT NULL,
    sha256         TEXT NOT NULL,
    size_bytes     BIGINT NOT NULL,
    line_count     INTEGER,
    uploaded_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    first_seen_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (machine_id, session_id)
);

CREATE INDEX IF NOT EXISTS idx_archive_machine_project ON archive_index(machine_id, project_dir);
CREATE INDEX IF NOT EXISTS idx_archive_session ON archive_index(session_id);
