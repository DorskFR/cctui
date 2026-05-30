-- Multi-adapter support: tag every session with the adapter (`claude-code`,
-- `codex`, …) that produced it. Legacy rows default to `claude-code` (the
-- only supported adapter pre-v0). The session id is already the primary key,
-- so no extra unique constraint is needed; the adapter_id is purely an
-- annotation used for client display and routing.

ALTER TABLE sessions ADD COLUMN IF NOT EXISTS adapter_id TEXT;

UPDATE sessions SET adapter_id = 'claude-code' WHERE adapter_id IS NULL;

ALTER TABLE sessions ALTER COLUMN adapter_id SET NOT NULL;
ALTER TABLE sessions ALTER COLUMN adapter_id SET DEFAULT 'claude-code';

CREATE INDEX IF NOT EXISTS idx_sessions_adapter ON sessions(adapter_id);
