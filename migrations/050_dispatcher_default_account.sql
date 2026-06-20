-- CCT-427: bind an OAuth account to a kube dispatcher.
--
-- A dispatcher is enrolled per-user (CCT-285/040) but carries no account
-- binding, so a dispatch with no explicit `account` cannot route the worker's
-- model traffic through the cctui gateway. Add an optional default account: a
-- dispatch with an empty `req.account` falls back to this account (mint a
-- session-scoped gateway token + inject the gateway env). An explicit
-- `req.account` still overrides it.
--
-- Forward-only and idempotent. The FK uses ON DELETE SET NULL so deleting an
-- oauth account simply unbinds the dispatcher rather than blocking the delete.
-- The optional provider hint disambiguates a name that exists across providers
-- (mirrors the explicit `provider` the dispatch path already accepts).

ALTER TABLE dispatchers
    ADD COLUMN IF NOT EXISTS default_account_id UUID
        REFERENCES oauth_accounts (id) ON DELETE SET NULL;

ALTER TABLE dispatchers
    ADD COLUMN IF NOT EXISTS default_account_provider TEXT;
