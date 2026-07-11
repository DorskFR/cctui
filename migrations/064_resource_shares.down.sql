-- Reverse CCT-531: restore the account-specific share table and fold the
-- account rows back out of the polymorphic table.
CREATE TABLE IF NOT EXISTS account_shares (
    account_id UUID        NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    user_id    UUID        NOT NULL REFERENCES users(id)          ON DELETE CASCADE,
    action     TEXT        NOT NULL DEFAULT 'use',
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (account_id, user_id, action)
);

CREATE INDEX IF NOT EXISTS idx_account_shares_user
    ON account_shares (user_id) WHERE revoked_at IS NULL;

INSERT INTO account_shares (account_id, user_id, action, granted_at, revoked_at)
SELECT resource_id, grantee_id, action, granted_at, revoked_at
FROM resource_shares
WHERE resource_type = 'account'
ON CONFLICT (account_id, user_id, action) DO NOTHING;

DROP TABLE IF EXISTS resource_shares;
