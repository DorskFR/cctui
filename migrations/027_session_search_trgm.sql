-- CCT-184: substring keyword search across full session transcripts.
-- pg_trgm gives index-accelerated `ILIKE '%kw%'` over a generated text
-- projection of each stream event's human-readable content (message text,
-- tool result content, tool name, tool input). This lets the archive view
-- find every session that ever mentioned a word (e.g. "kill", "delete"),
-- not just titles or the last message.
CREATE EXTENSION IF NOT EXISTS pg_trgm;

-- Stored projection of the searchable text in each event's JSONB payload.
-- Kept narrow on purpose (just the human-readable bits) so the trgm index
-- stays focused rather than matching raw JSON structure noise.
ALTER TABLE stream_events
    ADD COLUMN IF NOT EXISTS search_text TEXT
        GENERATED ALWAYS AS (
            coalesce(payload ->> 'text', '') || ' ' ||
            coalesce(payload ->> 'content', '') || ' ' ||
            coalesce(payload ->> 'tool', '') || ' ' ||
            coalesce((payload -> 'input')::text, '')
        ) STORED;

CREATE INDEX IF NOT EXISTS idx_stream_events_search_trgm
    ON stream_events USING gin (search_text gin_trgm_ops);
