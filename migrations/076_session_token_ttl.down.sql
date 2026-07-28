DROP INDEX IF EXISTS session_tokens_expires_at;
ALTER TABLE session_tokens DROP COLUMN IF EXISTS expires_at;
