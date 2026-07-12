-- CCT-594: project the already-ingested `ToolUse` firehose onto the session
-- list. `last_tool_at` (+ name) is bumped on every tool call and rolled up the
-- `parent_id` chain like `last_heartbeat`, so a grinding subagent lights up the
-- parent row; `tool_use_count` is the session's own per-turn running count
-- (reset on a fresh user prompt). Piggybacks on the existing heartbeat UPDATE.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS last_tool_at TIMESTAMPTZ;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS last_tool_name TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS tool_use_count INTEGER NOT NULL DEFAULT 0;
