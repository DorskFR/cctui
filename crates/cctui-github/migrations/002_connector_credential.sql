-- GH-CONN-1: give github.connectors an encrypted credential + config.
--
-- Runs with `search_path = github` (see `cctui_github::migrate`), so the
-- unqualified `connectors` below resolves to `github.connectors` created in 001.
-- The one-directional-FK invariant (docs/github-integration.md §7.2) is
-- unchanged: this migration only adds columns to a github.* table.
--
-- Secrets at rest: `encrypted_credential` and `encrypted_webhook_secret` hold
-- the vault-encrypted (crate::crypto XOR-vault, same key as oauth_accounts)
-- ciphertext. They are NEVER returned over the API — every read path masks the
-- credential and reports only whether a webhook secret is set.

ALTER TABLE connectors
    ADD COLUMN name                      TEXT  NOT NULL DEFAULT '',
    -- 'pat' | 'app_installation'
    ADD COLUMN credential_kind           TEXT  NOT NULL DEFAULT 'pat',
    -- Vault-encrypted GitHub credential (PAT or App installation token).
    ADD COLUMN encrypted_credential      TEXT  NOT NULL DEFAULT '',
    -- Vault-encrypted webhook signing secret; NULL = none configured.
    ADD COLUMN encrypted_webhook_secret  TEXT,
    -- owner/name slugs (or bare owner) this connector tracks.
    ADD COLUMN repos                     TEXT[] NOT NULL DEFAULT '{}';

-- Drop the placeholder defaults now that the columns exist; future inserts
-- always supply name/credential explicitly.
ALTER TABLE connectors
    ALTER COLUMN name DROP DEFAULT,
    ALTER COLUMN credential_kind DROP DEFAULT,
    ALTER COLUMN encrypted_credential DROP DEFAULT;

-- A user cannot have two connectors with the same name.
CREATE UNIQUE INDEX connectors_user_name_idx ON connectors (user_id, name);
