DROP INDEX IF EXISTS idx_auth_keys_passkey;
ALTER TABLE auth_keys DROP COLUMN IF EXISTS passkey_id;
