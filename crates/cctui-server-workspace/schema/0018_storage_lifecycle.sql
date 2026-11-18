CREATE TABLE storage_lifecycle_rules (
    action      TEXT PRIMARY KEY CHECK (action IN ('trim', 'drop')),
    enabled     BOOLEAN NOT NULL DEFAULT false,
    after_days  INT NOT NULL CHECK (after_days BETWEEN 1 AND 3650),
    above_bytes BIGINT CHECK (above_bytes >= 1024),
    updated_by  UUID,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((action = 'trim') = (above_bytes IS NOT NULL))
);

CREATE TABLE storage_lifecycle_cursors (
    name          TEXT NOT NULL,
    action        TEXT NOT NULL,
    rule          TEXT NOT NULL,
    last_event_id BIGINT NOT NULL DEFAULT 0,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (name, action)
);

CREATE TABLE storage_lifecycle_runs (
    id           BIGSERIAL PRIMARY KEY,
    name         TEXT NOT NULL,
    started_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at  TIMESTAMPTZ,
    rows_scanned BIGINT NOT NULL DEFAULT 0,
    rows_trimmed BIGINT NOT NULL DEFAULT 0,
    rows_dropped BIGINT NOT NULL DEFAULT 0,
    rows_skipped BIGINT NOT NULL DEFAULT 0,
    bytes_freed  BIGINT NOT NULL DEFAULT 0,
    error        TEXT
);
CREATE INDEX storage_lifecycle_runs_name ON storage_lifecycle_runs (name, id DESC);
