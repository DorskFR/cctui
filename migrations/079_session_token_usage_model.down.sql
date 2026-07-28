DROP INDEX IF EXISTS session_token_usage_created_idx;

ALTER TABLE session_token_usage DROP COLUMN IF EXISTS model;
