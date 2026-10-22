-- Cursor and counters of the walk that moves inline base64 sources out of
-- stream_events into daemon_blobs. Empty until the walk is enabled.
CREATE TABLE IF NOT EXISTS image_blob_backfill (
    name           TEXT PRIMARY KEY,
    last_event_id  BIGINT      NOT NULL,
    rows_rewritten BIGINT      NOT NULL DEFAULT 0,
    blob_bytes     BIGINT      NOT NULL DEFAULT 0,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
