SET LOCAL lock_timeout = '3s';
ALTER TABLE public.stream_events DROP COLUMN IF EXISTS search_text;
