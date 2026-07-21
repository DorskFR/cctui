-- CCT-739: content-addressed store for oversized embedded attachments (base64
-- images/screenshots in transcript tool_result/message payloads). The daemon
-- extracts a blob over 512 KiB, PUTs it here keyed by sha256, and replaces it in
-- the WS event with a {type:"cctui-blob", blob_id, size, media_type} marker so
-- the bytes leave the event stream. Hash-addressed + globally dedup'd (one row
-- per content hash regardless of session). Postgres bytea, same backend as
-- session_images (CCT-566).
CREATE TABLE daemon_blobs (
    hash        TEXT PRIMARY KEY,
    media_type  TEXT,
    byte_len    BIGINT NOT NULL,
    bytes       BYTEA NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
