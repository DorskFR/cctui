-- The passkey that minted a login key, so revoking the passkey revokes its
-- sessions. No FK: the id outlives the credential row for auditing.
ALTER TABLE auth_keys ADD COLUMN IF NOT EXISTS passkey_id UUID;
CREATE INDEX IF NOT EXISTS idx_auth_keys_passkey ON auth_keys (passkey_id)
    WHERE passkey_id IS NOT NULL;
