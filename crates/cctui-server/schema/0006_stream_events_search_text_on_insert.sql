-- search_text only serves search for events without chunks. Rows chunked on
-- ingest store '' so the trigram index takes no keys for them; every other
-- insert keeps the former generated value. Dropping the expression keeps the
-- stored values and does not rewrite the table.
SET LOCAL lock_timeout = '10s';
ALTER TABLE public.stream_events ALTER COLUMN search_text DROP EXPRESSION;

CREATE FUNCTION public.stream_events_search_text() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'INSERT' AND NEW.search_text IS NOT NULL THEN
        RETURN NEW;
    END IF;
    IF TG_OP = 'UPDATE' AND OLD.search_text = '' THEN
        NEW.search_text := '';
        RETURN NEW;
    END IF;
    NEW.search_text := COALESCE(NEW.payload ->> 'text', '') || ' '
        || COALESCE(NEW.payload ->> 'content', '') || ' '
        || COALESCE(NEW.payload ->> 'tool', '') || ' '
        || COALESCE((NEW.payload -> 'input')::text, '');
    RETURN NEW;
END
$$;

CREATE TRIGGER stream_events_search_text
    BEFORE INSERT OR UPDATE OF payload ON public.stream_events
    FOR EACH ROW EXECUTE FUNCTION public.stream_events_search_text();
