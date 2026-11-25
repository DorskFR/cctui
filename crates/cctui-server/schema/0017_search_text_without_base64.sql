-- The former search_text rule, minus base64 attachment data: a JSON `data`
-- string or a `;base64,` URL body of 200+ base64 characters is emptied.
-- Replaces functions only; stored rows are untouched.
CREATE FUNCTION public.stream_events_search_text_of(payload jsonb) RETURNS text
    LANGUAGE plpgsql IMMUTABLE PARALLEL SAFE
    AS $$
DECLARE
    s text := COALESCE(payload ->> 'text', '') || ' '
        || COALESCE(payload ->> 'content', '') || ' '
        || COALESCE(payload ->> 'tool', '') || ' '
        || COALESCE((payload -> 'input')::text, '');
BEGIN
    IF strpos(s, 'base64') = 0 THEN
        RETURN s;
    END IF;
    s := regexp_replace(s, '"data": "[A-Za-z0-9+/]{200}[^"]*"', '"data": ""', 'g');
    RETURN regexp_replace(s, ';base64,[A-Za-z0-9+/]{200}[A-Za-z0-9+/=]*', ';base64,', 'g');
END
$$;

CREATE OR REPLACE FUNCTION public.stream_events_search_text() RETURNS trigger
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
    NEW.search_text := public.stream_events_search_text_of(NEW.payload);
    RETURN NEW;
END
$$;
