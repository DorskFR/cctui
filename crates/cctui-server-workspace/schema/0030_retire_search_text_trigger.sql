-- The search_text column stays until no pod older than this migration runs:
-- those still name it in their inserts.
SET LOCAL lock_timeout = '3s';
DROP TRIGGER IF EXISTS stream_events_search_text ON public.stream_events;
DROP FUNCTION IF EXISTS public.stream_events_search_text();
DROP FUNCTION IF EXISTS public.stream_events_search_text_of(jsonb);
