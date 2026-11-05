-- A pending list makes the insert that overflows it merge every queued entry
-- into the index synchronously, and every search scan it linearly.
DO $$
DECLARE
    idx regclass := pg_catalog.to_regclass('idx_search_chunks_session_body_trgm');
BEGIN
    IF idx IS NOT NULL THEN
        EXECUTE pg_catalog.format('ALTER INDEX %s SET (fastupdate = off)', idx);
        PERFORM pg_catalog.gin_clean_pending_list(idx);
    END IF;
END
$$;
