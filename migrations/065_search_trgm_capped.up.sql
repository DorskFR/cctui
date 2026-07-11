-- Free-text session search matches only the first 8 KiB of each event's
-- search_text: search_text is unbounded (tool payloads reach 1.7 MB on dev)
-- and the uncapped trgm index forced the bitmap recheck to detoast every
-- candidate row, making cold searches take multiple seconds. The 8192 literal
-- must match SEARCH_TEXT_CAP in routes/sessions.rs — the query's ILIKE runs
-- against the same left() expression so this index serves it.
--
-- Already applied out-of-band (CREATE INDEX CONCURRENTLY) on the dev database;
-- IF NOT EXISTS makes this a no-op there and a real (fast, small-table) build
-- on fresh installs.
CREATE INDEX IF NOT EXISTS idx_stream_events_search_trgm_capped
    ON stream_events USING gin (left(search_text, 8192) gin_trgm_ops);

DROP INDEX IF EXISTS idx_stream_events_search_trgm;
