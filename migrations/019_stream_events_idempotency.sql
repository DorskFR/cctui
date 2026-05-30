-- Idempotency for stream_events (CCT-92).
--
-- The daemon's transcript tail can advance + persist its file offset
-- before the events it just parsed are confirmed shipped to the server.
-- A WS drop in that window loses events permanently. With a content-hash
-- uniqueness constraint we can have the daemon replay safely on every
-- (re)connect and the server silently de-dupes via ON CONFLICT DO NOTHING.

-- `digest()` lives in pgcrypto; ensure it's available before we reference it.
CREATE EXTENSION IF NOT EXISTS pgcrypto;

ALTER TABLE stream_events
    ADD COLUMN IF NOT EXISTS content_hash bytea
        GENERATED ALWAYS AS (digest(payload::text, 'sha256')) STORED;

CREATE UNIQUE INDEX IF NOT EXISTS stream_events_dedup_idx
    ON stream_events (session_id, event_type, content_hash);
