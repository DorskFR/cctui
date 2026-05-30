-- Trigger registry — stub. v0 reserves the table so adding an external
-- trigger (GitHub webhook, Slack mention, cron) post-v0 is purely additive.
-- The `/api/v1/triggers/{kind}` route returns 501 until consumers are wired.

CREATE TABLE IF NOT EXISTS triggers (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind         TEXT NOT NULL,
    config       JSONB NOT NULL DEFAULT '{}'::jsonb,
    secret_hash  TEXT,
    enabled      BOOLEAN NOT NULL DEFAULT FALSE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_triggers_user ON triggers(user_id);
