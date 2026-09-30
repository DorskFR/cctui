-- Names a quota probe in the server's probe registry for a compatible-endpoint
-- credential. The credential itself stays generic: this adds measurement
-- (windows, and therefore pace and soft limits), not a provider kind. NULL ⇒
-- the credential is unmeasured, which `account_pick` already reads as
-- `usage_known: false`.
ALTER TABLE account_providers ADD COLUMN IF NOT EXISTS usage_probe text;
