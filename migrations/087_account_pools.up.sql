-- Account pools: the declared set of accounts a session may run on.
--
-- Before pools there was no boundary. `auto_account` ranked *every* account the
-- caller could reach — their own and every one shared with them — and a live
-- session that ran out of allocation was rebound the same way. Nothing said
-- which accounts were interchangeable, so personal work could silently spill
-- onto a work credential and only be discovered after the fact.
--
-- A pool is that missing statement: "these accounts are interchangeable, pick
-- among them and never leave". It is the unit of both load balancing (which
-- member a launch binds) and failover (where a live session may be moved). A
-- session that binds no pool behaves exactly as it does today.
CREATE TABLE account_pools (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name       TEXT        NOT NULL,
    -- 'headroom': bind the member with the most allocation left (the old
    -- `auto_account` ranking, now scoped to the pool).
    -- 'ordered':  walk the members in `position` order, first one with room.
    strategy   TEXT        NOT NULL DEFAULT 'headroom',
    -- Whether a LIVE session bound to this pool may be moved between members
    -- when its account is refused. Off by default: joining a pool changes how
    -- a launch picks, never what happens mid-run, until this is set.
    failover   BOOLEAN     NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT account_pools_strategy CHECK (strategy IN ('headroom', 'ordered'))
);

-- Pools are addressed by name at spawn, so the name must be unambiguous per
-- user; case-insensitive, like the accounts a caller types by hand.
CREATE UNIQUE INDEX account_pools_user_name ON account_pools (user_id, lower(name));

CREATE TABLE account_pool_members (
    pool_id    UUID    NOT NULL REFERENCES account_pools(id) ON DELETE CASCADE,
    account_id UUID    NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    -- Only meaningful for the 'ordered' strategy; ties break on account name.
    position   INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (pool_id, account_id)
);

-- Revoking a share has to find the pools that referenced the account.
CREATE INDEX account_pool_members_account ON account_pool_members (account_id);

-- An owner's veto over accounts they lend out: with this false, only the owner
-- may enrol the account in a pool. A grantee can still launch on it explicitly
-- — what they cannot do is make it a silent overflow target for their own work.
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS pool_eligible BOOLEAN NOT NULL DEFAULT TRUE;

-- The pool a live session draws from. Remembered (not just the elected member)
-- so a long run can be rebound more than once, and so the UI can say which set
-- the session is allowed to move inside.
ALTER TABLE session_tokens
    ADD COLUMN IF NOT EXISTS pool_id UUID REFERENCES account_pools(id) ON DELETE SET NULL;

-- Every mid-session account move, kept as history. Account *names* rather than
-- ids: the row must stay readable after an account is deleted, and this is an
-- audit trail, not a foreign key. This table is the answer to "my sessions
-- moved and I only found out later".
CREATE TABLE session_account_rebinds (
    id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id   TEXT        NOT NULL,
    pool_id      UUID        REFERENCES account_pools(id) ON DELETE SET NULL,
    from_account TEXT        NOT NULL,
    to_account   TEXT        NOT NULL,
    -- 'pool' (elected inside the session's pool) or 'redirect' (an explicit
    -- account_redirects rule), so the UI can name the mechanism that moved it.
    reason       TEXT        NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX session_account_rebinds_session
    ON session_account_rebinds (session_id, created_at DESC);
