-- CCT-558: split accounts into identity (accounts) + provider credentials
-- (account_providers, née oauth_accounts).
--
-- Today one oauth_accounts row conflates the account identity ("alice") with a
-- single provider credential (anthropic | openai | *-compatible). This
-- restructures it into:
--
--   * `accounts`          — the identity: name, owner, env overrides, sharing.
--   * `account_providers` — one row per provider credential under an account:
--     tokens, base_url/auth_scheme/models (042), model_aliases (043), soft
--     limits (045), needs_reauth (058), settings_json (059), usage counters.
--
-- Backfill is dumb and lossless: each existing oauth_accounts row becomes one
-- `accounts` parent with one child. The parent REUSES the old row's uuid as its
-- own id, and the child keeps that same uuid too — so identity-level FKs
-- (account_shares.account_id, dispatchers.default_account_id) re-point to
-- accounts(id) without any value rewrite, and credential-level FKs
-- (session_tokens.account_id) stay valid on account_providers unchanged.
--
-- The CCT-538 per-account launch defaults (default_model / default_effort /
-- default_permission_mode) are dropped: superseded by per-(machine, cwd)
-- client memory (see the CCT-558 epic spec).
--
-- A generated `family` column + unique (account_id, family) enforces at most
-- one provider per family (anthropic | openai) per account, preserving the
-- CCT-508 same-family env-collision guard by construction.

-- 1. Identity parent. Owner semantics mirror oauth_accounts.user_id.
CREATE TABLE accounts (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT        NOT NULL,
    env_json   TEXT,       -- encrypted env-var blob (crate::crypto), write-only; moved from 059
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- oauth_accounts was unique on (user_id, name, provider); the identity has no
-- provider, so the parent is unique on (user_id, name).
CREATE UNIQUE INDEX accounts_user_name ON accounts (user_id, name);

-- 2. Backfill: one parent per existing row, id := old row id. Where a user has
-- the same name across providers (would collide on the new unique index),
-- disambiguate with a provider suffix; single-name rows keep their name as-is.
INSERT INTO accounts (id, user_id, name, env_json, created_at)
SELECT id,
       user_id,
       CASE WHEN COUNT(*) OVER (PARTITION BY user_id, name) > 1
            THEN name || ' (' || provider || ')'
            ELSE name
       END,
       env_json,
       created_at
FROM oauth_accounts;

-- 3. Rename the credential table and link each child to its parent (same uuid).
ALTER TABLE oauth_accounts RENAME TO account_providers;

ALTER TABLE account_providers ADD COLUMN account_id UUID;
UPDATE account_providers SET account_id = id;
ALTER TABLE account_providers
    ALTER COLUMN account_id SET NOT NULL,
    ADD CONSTRAINT account_providers_account_id_fkey
        FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE;

-- 4. Columns that moved to the parent (env_json) or died with CCT-538
-- (launch defaults). Dropping `name` also drops the old
-- oauth_accounts_user_name_provider unique index.
ALTER TABLE account_providers
    DROP COLUMN name,
    DROP COLUMN env_json,
    DROP COLUMN default_model,
    DROP COLUMN default_effort,
    DROP COLUMN default_permission_mode;

-- 5. One provider per family per account. Family follows
-- Family::from_provider: anything containing "openai" is the openai family
-- ('openai', 'openai-compatible'); everything else is anthropic.
ALTER TABLE account_providers
    ADD COLUMN family TEXT GENERATED ALWAYS AS (
        CASE WHEN provider LIKE '%openai%' THEN 'openai' ELSE 'anthropic' END
    ) STORED;

CREATE UNIQUE INDEX account_providers_account_family
    ON account_providers (account_id, family);

-- 6. Identity-level FKs re-point to accounts(id); values are unchanged because
-- parent ids reuse the old row ids. Credential-level FKs (session_tokens)
-- stay on account_providers — the table rename kept them valid.
ALTER TABLE account_shares
    DROP CONSTRAINT account_shares_account_id_fkey,
    ADD CONSTRAINT account_shares_account_id_fkey
        FOREIGN KEY (account_id) REFERENCES accounts(id) ON DELETE CASCADE;

ALTER TABLE dispatchers
    DROP CONSTRAINT dispatchers_default_account_id_fkey,
    ADD CONSTRAINT dispatchers_default_account_id_fkey
        FOREIGN KEY (default_account_id) REFERENCES accounts(id) ON DELETE SET NULL;
