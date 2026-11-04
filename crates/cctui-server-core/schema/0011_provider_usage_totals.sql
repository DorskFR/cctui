-- Running token totals per provider and model: the sum of session_usage_totals
-- over every session_tokens row bridging a session to the provider, kept by
-- triggers on both tables. No foreign key: a provider delete cascades its
-- session_tokens, whose triggers still write here.
CREATE TABLE IF NOT EXISTS public.account_provider_usage_totals (
    provider_id uuid NOT NULL,
    model text NOT NULL,
    input_tokens bigint DEFAULT 0 NOT NULL,
    output_tokens bigint DEFAULT 0 NOT NULL,
    cache_read_tokens bigint DEFAULT 0 NOT NULL,
    cache_creation_tokens bigint DEFAULT 0 NOT NULL,
    CONSTRAINT account_provider_usage_totals_pkey PRIMARY KEY (provider_id, model)
);

CREATE OR REPLACE FUNCTION public.account_provider_usage_bump(
    p_session text, p_model text, d_input bigint, d_output bigint, d_cache_read bigint,
    d_cache_creation bigint) RETURNS void
    LANGUAGE sql
    AS $$
    INSERT INTO account_provider_usage_totals AS t
        (provider_id, model, input_tokens, output_tokens, cache_read_tokens,
         cache_creation_tokens)
    SELECT st.account_id, p_model, count(*) * d_input, count(*) * d_output,
           count(*) * d_cache_read, count(*) * d_cache_creation
    FROM session_tokens st
    WHERE st.session_id = p_session
    GROUP BY st.account_id
    ORDER BY st.account_id
    ON CONFLICT (provider_id, model) DO UPDATE SET
        input_tokens          = t.input_tokens          + EXCLUDED.input_tokens,
        output_tokens         = t.output_tokens         + EXCLUDED.output_tokens,
        cache_read_tokens     = t.cache_read_tokens     + EXCLUDED.cache_read_tokens,
        cache_creation_tokens = t.cache_creation_tokens + EXCLUDED.cache_creation_tokens;
$$;

CREATE OR REPLACE FUNCTION public.account_provider_usage_from_session() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP = 'UPDATE' AND OLD.session_id = NEW.session_id AND OLD.model = NEW.model THEN
        PERFORM account_provider_usage_bump(NEW.session_id, NEW.model,
            NEW.input_tokens - OLD.input_tokens, NEW.output_tokens - OLD.output_tokens,
            NEW.cache_read_tokens - OLD.cache_read_tokens,
            NEW.cache_creation_tokens - OLD.cache_creation_tokens);
        RETURN NULL;
    END IF;
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        PERFORM account_provider_usage_bump(OLD.session_id, OLD.model,
            -OLD.input_tokens, -OLD.output_tokens, -OLD.cache_read_tokens,
            -OLD.cache_creation_tokens);
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        PERFORM account_provider_usage_bump(NEW.session_id, NEW.model,
            NEW.input_tokens, NEW.output_tokens, NEW.cache_read_tokens,
            NEW.cache_creation_tokens);
    END IF;
    RETURN NULL;
END;
$$;

CREATE OR REPLACE FUNCTION public.account_provider_usage_from_token() RETURNS trigger
    LANGUAGE plpgsql
    AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        INSERT INTO account_provider_usage_totals AS t
            (provider_id, model, input_tokens, output_tokens, cache_read_tokens,
             cache_creation_tokens)
        SELECT OLD.account_id, stu.model, -stu.input_tokens, -stu.output_tokens,
               -stu.cache_read_tokens, -stu.cache_creation_tokens
        FROM session_usage_totals stu
        WHERE stu.session_id = OLD.session_id
        ORDER BY stu.model
        ON CONFLICT (provider_id, model) DO UPDATE SET
            input_tokens          = t.input_tokens          + EXCLUDED.input_tokens,
            output_tokens         = t.output_tokens         + EXCLUDED.output_tokens,
            cache_read_tokens     = t.cache_read_tokens     + EXCLUDED.cache_read_tokens,
            cache_creation_tokens = t.cache_creation_tokens + EXCLUDED.cache_creation_tokens;
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        INSERT INTO account_provider_usage_totals AS t
            (provider_id, model, input_tokens, output_tokens, cache_read_tokens,
             cache_creation_tokens)
        SELECT NEW.account_id, stu.model, stu.input_tokens, stu.output_tokens,
               stu.cache_read_tokens, stu.cache_creation_tokens
        FROM session_usage_totals stu
        WHERE stu.session_id = NEW.session_id
        ORDER BY stu.model
        ON CONFLICT (provider_id, model) DO UPDATE SET
            input_tokens          = t.input_tokens          + EXCLUDED.input_tokens,
            output_tokens         = t.output_tokens         + EXCLUDED.output_tokens,
            cache_read_tokens     = t.cache_read_tokens     + EXCLUDED.cache_read_tokens,
            cache_creation_tokens = t.cache_creation_tokens + EXCLUDED.cache_creation_tokens;
    END IF;
    RETURN NULL;
END;
$$;

LOCK TABLE public.session_tokens, public.session_usage_totals IN SHARE ROW EXCLUSIVE MODE;

DROP TRIGGER IF EXISTS account_provider_usage_from_session ON public.session_usage_totals;
DROP TRIGGER IF EXISTS account_provider_usage_from_token ON public.session_tokens;
DROP TRIGGER IF EXISTS account_provider_usage_from_token_rebind ON public.session_tokens;

CREATE TRIGGER account_provider_usage_from_session
    AFTER INSERT OR DELETE OR UPDATE ON public.session_usage_totals
    FOR EACH ROW EXECUTE FUNCTION public.account_provider_usage_from_session();

CREATE TRIGGER account_provider_usage_from_token
    AFTER INSERT OR DELETE ON public.session_tokens
    FOR EACH ROW EXECUTE FUNCTION public.account_provider_usage_from_token();

CREATE TRIGGER account_provider_usage_from_token_rebind
    AFTER UPDATE OF session_id, account_id ON public.session_tokens
    FOR EACH ROW
    WHEN (OLD.session_id IS DISTINCT FROM NEW.session_id
          OR OLD.account_id IS DISTINCT FROM NEW.account_id)
    EXECUTE FUNCTION public.account_provider_usage_from_token();

DELETE FROM public.account_provider_usage_totals;
INSERT INTO public.account_provider_usage_totals
    (provider_id, model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens)
SELECT st.account_id, stu.model, SUM(stu.input_tokens)::bigint, SUM(stu.output_tokens)::bigint,
       SUM(stu.cache_read_tokens)::bigint, SUM(stu.cache_creation_tokens)::bigint
FROM public.session_tokens st
JOIN public.session_usage_totals stu ON stu.session_id = st.session_id
GROUP BY st.account_id, stu.model;
