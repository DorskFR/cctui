DROP INDEX IF EXISTS session_tokens_service_by_account;
ALTER TABLE session_tokens DROP COLUMN IF EXISTS label;
ALTER TABLE session_tokens DROP COLUMN IF EXISTS kind;
