-- Running per-(session, model) token totals, so the sessions list and the
-- accounts list read one row per session and model instead of summing every
-- `session_token_usage` row on each fetch. Kept exact by a row trigger on
-- every write to `session_token_usage`. `model` is '' for NULL-model rows.
CREATE TABLE IF NOT EXISTS session_usage_totals (
    session_id            text   NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    model                 text   NOT NULL DEFAULT '',
    input_tokens          bigint NOT NULL DEFAULT 0,
    output_tokens         bigint NOT NULL DEFAULT 0,
    cache_read_tokens     bigint NOT NULL DEFAULT 0,
    cache_creation_tokens bigint NOT NULL DEFAULT 0,
    PRIMARY KEY (session_id, model)
);

CREATE OR REPLACE FUNCTION session_usage_totals_apply() RETURNS trigger AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        UPDATE session_usage_totals SET
            input_tokens          = input_tokens          - OLD.input_tokens,
            output_tokens         = output_tokens         - OLD.output_tokens,
            cache_read_tokens     = cache_read_tokens     - OLD.cache_read_tokens,
            cache_creation_tokens = cache_creation_tokens - OLD.cache_creation_tokens
        WHERE session_id = OLD.session_id AND model = COALESCE(OLD.model, '');
    END IF;
    IF TG_OP IN ('INSERT', 'UPDATE') THEN
        INSERT INTO session_usage_totals AS t
            (session_id, model, input_tokens, output_tokens, cache_read_tokens,
             cache_creation_tokens)
        VALUES (NEW.session_id, COALESCE(NEW.model, ''), NEW.input_tokens, NEW.output_tokens,
                NEW.cache_read_tokens, NEW.cache_creation_tokens)
        ON CONFLICT (session_id, model) DO UPDATE SET
            input_tokens          = t.input_tokens          + EXCLUDED.input_tokens,
            output_tokens         = t.output_tokens         + EXCLUDED.output_tokens,
            cache_read_tokens     = t.cache_read_tokens     + EXCLUDED.cache_read_tokens,
            cache_creation_tokens = t.cache_creation_tokens + EXCLUDED.cache_creation_tokens;
    END IF;
    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

-- Blocks writers (not readers) until commit, so no insert lands between the
-- trigger going live and the backfill snapshot.
LOCK TABLE session_token_usage IN SHARE ROW EXCLUSIVE MODE;

DROP TRIGGER IF EXISTS session_usage_totals_apply ON session_token_usage;
CREATE TRIGGER session_usage_totals_apply
    AFTER INSERT OR UPDATE OR DELETE ON session_token_usage
    FOR EACH ROW EXECUTE FUNCTION session_usage_totals_apply();

TRUNCATE session_usage_totals;
INSERT INTO session_usage_totals
    (session_id, model, input_tokens, output_tokens, cache_read_tokens, cache_creation_tokens)
SELECT session_id, COALESCE(model, ''), SUM(input_tokens), SUM(output_tokens),
       SUM(cache_read_tokens), SUM(cache_creation_tokens)
FROM session_token_usage
GROUP BY session_id, COALESCE(model, '');
