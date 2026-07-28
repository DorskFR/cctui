-- Short-TTL gateway session tokens (CCT-503). The minted token is the only
-- copy that ever leaves the server (worker env + claude-daemon memory), so an
-- expiry bounds its blast radius: an abandoned/zombie worker's token self-
-- expires and the gateway refuses it. NULL = legacy row minted before this
-- column existed → never expires (no regression). Fresh mints always stamp it.
ALTER TABLE session_tokens ADD COLUMN IF NOT EXISTS expires_at TIMESTAMPTZ;

-- The gateway auth path filters live-and-unexpired tokens by hash; keep the
-- expiry visible to that lookup.
CREATE INDEX IF NOT EXISTS session_tokens_expires_at
    ON session_tokens (expires_at)
    WHERE revoked_at IS NULL;
