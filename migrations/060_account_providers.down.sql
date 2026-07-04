-- CCT-558 down: merge the identity parent back into the credential table and
-- restore the pre-060 oauth_accounts shape. Lossless for the
-- single-provider-per-account case (the only shape the up migration creates);
-- a disambiguating name suffix added by the up backfill (duplicate names) is
-- kept as-is, and manually merged multi-provider accounts flatten back to one
-- row per provider sharing the parent's name.

-- 1. Restore the columns that moved to the parent / were dropped.
ALTER TABLE account_providers
    ADD COLUMN name                    TEXT,
    ADD COLUMN env_json                TEXT,
    ADD COLUMN default_model           TEXT,
    ADD COLUMN default_effort          TEXT,
    ADD COLUMN default_permission_mode TEXT;

UPDATE account_providers ap
SET name     = a.name,
    env_json = a.env_json
FROM accounts a
WHERE ap.account_id = a.id;

ALTER TABLE account_providers ALTER COLUMN name SET NOT NULL;

-- 2. Re-point identity-level FKs at the credential table (values unchanged:
-- parent and child share the same uuid).
ALTER TABLE account_shares DROP CONSTRAINT account_shares_account_id_fkey;
ALTER TABLE dispatchers    DROP CONSTRAINT dispatchers_default_account_id_fkey;

-- 3. Drop the parent linkage + family guard and rename back.
DROP INDEX account_providers_account_family;
ALTER TABLE account_providers
    DROP COLUMN family,
    DROP COLUMN account_id;

ALTER TABLE account_providers RENAME TO oauth_accounts;

-- 4. Restore the pre-060 uniqueness rule and FKs.
CREATE UNIQUE INDEX oauth_accounts_user_name_provider
    ON oauth_accounts (user_id, name, provider);

ALTER TABLE account_shares
    ADD CONSTRAINT account_shares_account_id_fkey
        FOREIGN KEY (account_id) REFERENCES oauth_accounts(id) ON DELETE CASCADE;

ALTER TABLE dispatchers
    ADD CONSTRAINT dispatchers_default_account_id_fkey
        FOREIGN KEY (default_account_id) REFERENCES oauth_accounts(id) ON DELETE SET NULL;

DROP TABLE accounts;
