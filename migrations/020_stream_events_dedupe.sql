-- Dedupe stream_events for the idempotency unique index (CCT-92).
--
-- Migration 019 created the unique index, but databases that already had
-- duplicate rows from before idempotency landed couldn't apply it (the
-- CREATE UNIQUE INDEX would fail). For prod the index was already built
-- successfully against then-clean data; on fresh DBs we now ensure dedupe
-- happens before the index by dropping + recreating it idempotently.

DROP INDEX IF EXISTS stream_events_dedup_idx;

DELETE FROM stream_events s
USING stream_events t
WHERE s.session_id = t.session_id
  AND s.event_type = t.event_type
  AND s.content_hash = t.content_hash
  AND s.id > t.id;

CREATE UNIQUE INDEX stream_events_dedup_idx
    ON stream_events (session_id, event_type, content_hash);
