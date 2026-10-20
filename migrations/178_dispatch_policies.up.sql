-- Admin-defined per-user dispatch policy. A user without a row dispatches as
-- before; a row restricts every listed dimension (an empty list leaves that
-- dimension open) and grants the named dispatchers across owners.
CREATE TABLE IF NOT EXISTS dispatch_policies (
    user_id             UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    dispatchers         TEXT[] NOT NULL DEFAULT '{}',
    profiles            TEXT[] NOT NULL DEFAULT '{}',
    adapters            TEXT[] NOT NULL DEFAULT '{}',
    models              TEXT[] NOT NULL DEFAULT '{}',
    context_packs       JSONB NOT NULL DEFAULT '[]'::jsonb,
    max_timeout_minutes INTEGER CHECK (max_timeout_minutes IS NULL OR max_timeout_minutes > 0),
    max_concurrent      INTEGER CHECK (max_concurrent IS NULL OR max_concurrent > 0),
    daily_cap           INTEGER CHECK (daily_cap IS NULL OR daily_cap > 0),
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Every dispatch a policy-bound user forwarded, for the daily cap: ephemeral
-- keys are deleted once expired, so they cannot carry the day's count.
CREATE TABLE IF NOT EXISTS dispatch_policy_uses (
    session_id    TEXT PRIMARY KEY,
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    dispatched_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS dispatch_policy_uses_by_user_day
    ON dispatch_policy_uses (user_id, dispatched_at);
