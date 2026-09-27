-- Admin-installed runtime plugins. The archive bytes live in the
-- content-addressed daemon_blobs store; the row carries the validated manifest
-- and the instance-wide toggle.
CREATE TABLE plugins (
    id           TEXT PRIMARY KEY,
    version      TEXT NOT NULL,
    manifest     JSONB NOT NULL,
    archive_hash TEXT NOT NULL,
    enabled      BOOLEAN NOT NULL DEFAULT false,
    installed_by UUID REFERENCES users(id) ON DELETE SET NULL,
    installed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
