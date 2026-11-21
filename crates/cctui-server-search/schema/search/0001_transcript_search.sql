-- Transcript search: the trigram extensions and the indexes that use them.
-- Each object is created only when absent, so a database that already has
-- them takes no lock and changes nothing.
CREATE EXTENSION IF NOT EXISTS btree_gin WITH SCHEMA public;
CREATE EXTENSION IF NOT EXISTS pg_trgm WITH SCHEMA public;

SET LOCAL lock_timeout = '10s';
DO $$
BEGIN
    IF pg_catalog.to_regclass('public.idx_search_chunks_session_body_trgm') IS NULL THEN
        CREATE INDEX idx_search_chunks_session_body_trgm ON public.search_chunks USING gin (session_id, body public.gin_trgm_ops) WITH (fastupdate=off);
    END IF;
    IF pg_catalog.to_regclass('public.idx_stream_events_session_search_trgm') IS NULL THEN
        CREATE INDEX idx_stream_events_session_search_trgm ON public.stream_events USING gin (session_id, "left"(search_text, 8192) public.gin_trgm_ops) WITH (fastupdate=off);
    END IF;
END
$$;
