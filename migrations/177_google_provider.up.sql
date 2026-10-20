-- `google` is its own family: a Gemini API key next to the other credentials
-- on one account. The generated column is derived, so drop + re-add keeps
-- every existing row's family and only rows that never existed get the new
-- value: nothing can collide in the rebuilt unique index.

DROP INDEX account_providers_account_family;

ALTER TABLE account_providers DROP COLUMN family;

ALTER TABLE account_providers
    ADD COLUMN family TEXT GENERATED ALWAYS AS (
        CASE WHEN provider = 'fireworks' THEN 'fireworks'
             WHEN provider = 'google' THEN 'google'
             WHEN provider LIKE '%openai%' THEN 'openai'
             ELSE 'anthropic'
        END
    ) STORED;

CREATE UNIQUE INDEX account_providers_account_family
    ON account_providers (account_id, family);
