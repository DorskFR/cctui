-- Make transcript-line dedupe independent of turn_id (CCT-1382).
--
-- Migration 121 widened the dedup key with `COALESCE(turn_id, nil)` on the
-- assumption that "a replay of the same turn still carries the same turn_id".
-- That does not survive a daemon restart: the daemon holds turn ids in memory
-- only, so a re-tailed user line comes back with `turn_id = NULL` while the
-- stored row carries the real one. The two never conflict, the replay is
-- inserted, and — because `newly_inserted` gates the broadcast — it is also
-- re-streamed. Only user rows are affected; assistant and tool rows have no
-- turn_id, so their replays still collapse on nil = nil.
--
-- The stable identity of a user line is `payload->>'line_id'`, the transcript
-- record's own uuid (`with_line_id` in the daemon's transcript parser). It is
-- read from the line, never minted at emit time, so a replay reproduces it.
-- Keying on it gives back the replay idempotency of CCT-92/CCT-171 without
-- depending on turn_id.
--
-- `content_hash` stays in the key. Two different lines can share a `line_id`:
-- the `queued_command` attachment borrows its source line's uuid, and a
-- stream-json frame with no uuid falls back to its `timestamp`, which two
-- records can share. Those must not erase one another, and keeping the hash
-- costs nothing for the bug being fixed — the duplicate pair differs only in
-- turn_id, so it hashes identically either way.
--
-- 121's `stream_events_dedup_turn_idx` is kept: it is the only key rows
-- without a `line_id` have, and it is what still lets two genuinely distinct
-- sends of the same text coexist. With both indexes live the ingest inserts
-- use a bare `ON CONFLICT DO NOTHING` so either one can suppress a row.
--
-- On a populated database build the index out of band with CONCURRENTLY first;
-- the IF NOT EXISTS below is then a no-op. This file holds several statements
-- and so cannot use CONCURRENTLY itself (see migration 121).

UPDATE stream_events keep
SET turn_id = dup.turn_id
FROM stream_events dup
WHERE keep.turn_id IS NULL
  AND dup.turn_id IS NOT NULL
  AND keep.session_id = dup.session_id
  AND keep.event_type = dup.event_type
  AND keep.content_hash = dup.content_hash
  AND keep.payload ? 'line_id'
  AND dup.payload ? 'line_id'
  AND keep.payload->>'line_id' = dup.payload->>'line_id'
  AND keep.id < dup.id;

DELETE FROM stream_events s
USING stream_events t
WHERE s.session_id = t.session_id
  AND s.event_type = t.event_type
  AND s.content_hash = t.content_hash
  AND s.payload ? 'line_id'
  AND t.payload ? 'line_id'
  AND s.payload->>'line_id' = t.payload->>'line_id'
  AND s.id > t.id;

CREATE UNIQUE INDEX IF NOT EXISTS stream_events_dedup_line_idx
    ON stream_events (
        session_id,
        event_type,
        content_hash,
        (payload->>'line_id')
    )
    WHERE payload ? 'line_id';
