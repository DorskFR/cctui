-- CCT-594 down.
ALTER TABLE sessions DROP COLUMN IF EXISTS tool_use_count;
ALTER TABLE sessions DROP COLUMN IF EXISTS last_tool_name;
ALTER TABLE sessions DROP COLUMN IF EXISTS last_tool_at;
