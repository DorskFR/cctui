-- CCT-940 down.
ALTER TABLE sessions DROP COLUMN IF EXISTS todo_updated_at;
ALTER TABLE sessions DROP COLUMN IF EXISTS todos;
