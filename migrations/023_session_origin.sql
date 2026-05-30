-- CCT-107: track the origin of a session so dispatcher-launched ones can
-- be distinguished from daemon-spawned and channel-registered sessions.
-- Values: 'k8s_job', 'daemon_spawn', 'channel_register' (or NULL for legacy).
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS origin TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS dispatch_handle TEXT;

CREATE INDEX IF NOT EXISTS idx_sessions_origin ON sessions(origin) WHERE origin IS NOT NULL;
