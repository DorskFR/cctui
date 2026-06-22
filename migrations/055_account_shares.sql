-- CCT-458: share an oauth_account with another user without transferring
-- ownership. A live grant row lets a NON-owner resolve/use the account on the
-- gateway + dispatch path — e.g. the `automation` dispatch-user routing workers through
-- a `dorsk`-owned account (passed as `req.account`), so nothing is hardcoded in
-- cctui. This is the CCT-422 resource-sharing seam made concrete; ownership,
-- mutation (edit/delete), and the account's own auth stay with the owner.
CREATE TABLE IF NOT EXISTS account_shares (
    account_id UUID        NOT NULL REFERENCES oauth_accounts(id) ON DELETE CASCADE,
    user_id    UUID        NOT NULL REFERENCES users(id)          ON DELETE CASCADE,
    action     TEXT        NOT NULL DEFAULT 'use',
    granted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    PRIMARY KEY (account_id, user_id, action)
);

-- Hot path: "does user X have a live grant on account A" (resolution) and
-- "which accounts are shared to user X" (listing).
CREATE INDEX IF NOT EXISTS idx_account_shares_user
    ON account_shares (user_id) WHERE revoked_at IS NULL;
