-- CCT-940: project the agent's task list (claude `TodoWrite`, codex
-- `update_plan`) onto the session row so the list can show `3/7` + the
-- in-progress step without replaying the transcript. `todos` holds the
-- normalized array (`{content, status, active_form}`); the whole list is
-- rewritten on every write, so there is nothing to merge.
--
-- Unlike `last_tool_at` (068) this is written LEAF-ONLY and never rolled up the
-- `parent_id` chain: each subagent keeps its own list, so a child's write must
-- not clobber the parent's.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS todos JSONB;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS todo_updated_at TIMESTAMPTZ;
