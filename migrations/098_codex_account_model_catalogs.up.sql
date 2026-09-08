-- Codex model catalog fetched server-side from the ChatGPT backend with an
-- account's own OAuth credential. Model availability is an account entitlement,
-- so this is keyed by the provider credential that holds the OAuth; the
-- per-machine `codex_model_catalogs` stays as the fallback for accounts we hold
-- no OAuth for.
CREATE TABLE codex_account_model_catalogs (
    provider_id UUID        PRIMARY KEY REFERENCES account_providers(id) ON DELETE CASCADE,
    catalog     JSONB       NOT NULL,
    fetched_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
