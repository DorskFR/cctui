DROP INDEX IF EXISTS session_tokens_user_live;
ALTER TABLE session_tokens DROP COLUMN IF EXISTS user_id;
