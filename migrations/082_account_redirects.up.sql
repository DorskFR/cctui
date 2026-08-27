-- Per-user, optionally expiring launch-time overrides:
--   to_account set  ⇒ new sessions asking for from_account bind the target
--   to_model set    ⇒ new sessions on this account flip the requested model
-- Exactly one of the two per rule; live sessions are never touched.
CREATE TABLE account_redirects (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID        NOT NULL REFERENCES users(id)    ON DELETE CASCADE,
    from_account UUID        NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    to_account   UUID                 REFERENCES accounts(id) ON DELETE CASCADE,
    family       TEXT        NOT NULL,
    match_model  TEXT,
    to_model     TEXT,
    expires_at   TIMESTAMPTZ,
    reason       TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT account_redirects_one_target
        CHECK ((to_account IS NULL) <> (to_model IS NULL)),
    CONSTRAINT account_redirects_not_identity
        CHECK (to_account IS DISTINCT FROM from_account),
    CONSTRAINT account_redirects_match_needs_model
        CHECK (match_model IS NULL OR to_model IS NOT NULL)
);

-- Re-arming a rule for the same source overwrites instead of stacking.
CREATE UNIQUE INDEX account_redirects_uniq
    ON account_redirects (user_id, from_account, family, COALESCE(match_model, ''));

CREATE INDEX account_redirects_by_user ON account_redirects (user_id);
