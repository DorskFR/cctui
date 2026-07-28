-- CCT-754: Fireworks as a first-class provider kind.
--
-- `fireworks` speaks the OpenAI wire protocol but is NOT the openai family: it
-- carries its own credential (a static `fw_...` bearer), its own upstream, and
-- its own worker env pair, so an account may hold a codex credential AND a
-- Fireworks credential at once. That means the generated `family` column — and
-- with it the `UNIQUE (account_id, family)` one-provider-per-family rule — has
-- to grow a third value rather than fold fireworks into 'openai'.
--
-- The generated expression can't be altered in place; drop + re-add (the column
-- is derived, so no data is lost) and rebuild the unique index it feeds.

DROP INDEX account_providers_account_family;

ALTER TABLE account_providers DROP COLUMN family;

ALTER TABLE account_providers
    ADD COLUMN family TEXT GENERATED ALWAYS AS (
        CASE WHEN provider = 'fireworks' THEN 'fireworks'
             WHEN provider LIKE '%openai%' THEN 'openai'
             ELSE 'anthropic'
        END
    ) STORED;

CREATE UNIQUE INDEX account_providers_account_family
    ON account_providers (account_id, family);

-- Per-provider gateway settings, distinct from `settings_json` (which is the
-- validated claude-code harness settings blob): request-shaping knobs the
-- gateway applies on the way upstream, e.g. Fireworks' cache/session affinity
-- and `context_length_exceeded_behavior`. Seeded with defaults at create.
ALTER TABLE account_providers ADD COLUMN provider_settings JSONB;
