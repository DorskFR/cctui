-- One row per forwarded run: a session id re-dispatched onto a fresh run
-- counts again toward the daily cap.
ALTER TABLE dispatch_policy_uses DROP CONSTRAINT IF EXISTS dispatch_policy_uses_pkey;
ALTER TABLE dispatch_policy_uses ADD COLUMN IF NOT EXISTS id BIGSERIAL PRIMARY KEY;

CREATE INDEX IF NOT EXISTS dispatch_policy_uses_by_session
    ON dispatch_policy_uses (session_id);
