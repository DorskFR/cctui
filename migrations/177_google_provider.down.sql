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
