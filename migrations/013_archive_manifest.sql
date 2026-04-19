CREATE TABLE IF NOT EXISTS archive_manifest (
    machine_id    UUID        NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    project_dir   TEXT        NOT NULL,
    session_id    TEXT        NOT NULL,
    size_bytes    BIGINT      NOT NULL,
    mtime         TIMESTAMPTZ NOT NULL,
    reported_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (machine_id, session_id)
);

CREATE INDEX IF NOT EXISTS idx_manifest_machine_project ON archive_manifest(machine_id, project_dir);
