-- Dedupe transcript lines on `payload->>'line_id'`, the transcript record's
-- own uuid, instead of relying on turn_id: the daemon keeps turn ids in memory
-- only, so a line re-tailed after a restart comes back with `turn_id = NULL`
-- and slips past `stream_events_dedup_turn_idx`.
--
-- `content_hash` stays in the key because two different lines can share a
-- `line_id` (a `queued_command` attachment borrows its source line's uuid).
--
-- Duplicates are removed by deletion only, keeping the row that carries a
-- turn_id and otherwise the lowest id. Copying the replay's turn_id onto the
-- kept row instead would collide with `stream_events_dedup_turn_idx` while the
-- replay still exists.

WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY session_id, event_type, content_hash, payload->>'line_id'
               ORDER BY (turn_id IS NULL), id
           ) AS rn
    FROM stream_events
    WHERE payload ? 'line_id'
)
DELETE FROM stream_events s
USING ranked r
WHERE s.id = r.id
  AND r.rn > 1;

CREATE UNIQUE INDEX IF NOT EXISTS stream_events_dedup_line_idx
    ON stream_events (
        session_id,
        event_type,
        content_hash,
        (payload->>'line_id')
    )
    WHERE payload ? 'line_id';
