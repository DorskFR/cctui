-- A pending list makes the insert that overflows it merge every queued entry
-- into the index synchronously, and every search scan it linearly.
ALTER INDEX public.idx_search_chunks_session_body_trgm SET (fastupdate = off);
SELECT public.gin_clean_pending_list('public.idx_search_chunks_session_body_trgm'::regclass);
