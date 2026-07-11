-- CCT-531: generalize account_shares (CCT-458/510) into ONE polymorphic
-- "shareable resource" table. A live grant row lets a NON-owner `use` a
-- resource (account | machine | dispatcher | context_pack) without transferring
-- ownership. Owner stays derived from the owning resource's own `user_id` (no
-- denormalized owner_id — avoids drift); `resource_id` is intentionally NOT a
-- hard FK (polymorphic), integrity is enforced by the owning row's ON DELETE +
-- a cleanup on resource delete. Grants confer `use` only, never re-sharing.
CREATE TABLE IF NOT EXISTS resource_shares (
    resource_type TEXT        NOT NULL,
    resource_id   UUID        NOT NULL,
    grantee_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    action        TEXT        NOT NULL DEFAULT 'use',
    granted_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at    TIMESTAMPTZ,
    PRIMARY KEY (resource_type, resource_id, grantee_id, action)
);

-- Hot path: "which resources are shared to user X" (listing) and the per-object
-- grant lookup in Resource::authorize / gateway resolution.
CREATE INDEX IF NOT EXISTS idx_resource_shares_grantee
    ON resource_shares (grantee_id) WHERE revoked_at IS NULL;
CREATE INDEX IF NOT EXISTS idx_resource_shares_resource
    ON resource_shares (resource_type, resource_id) WHERE revoked_at IS NULL;

-- Data-migration: fold the bespoke account_shares rows in as accounts, then drop
-- the old table. The prior grants keep their granted_at / revoked_at / action.
INSERT INTO resource_shares (resource_type, resource_id, grantee_id, action, granted_at, revoked_at)
SELECT 'account', account_id, user_id, action, granted_at, revoked_at
FROM account_shares
ON CONFLICT (resource_type, resource_id, grantee_id, action) DO NOTHING;

DROP TABLE IF EXISTS account_shares;
