-- Dropping the index is enough: the rows it collapsed are gone, and 121's
-- turn-keyed index is untouched. The turn_id backfill is deliberately not
-- reverted — it made stored rows more accurate, not less.

DROP INDEX IF EXISTS stream_events_dedup_line_idx;
