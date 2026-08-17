-- Per-(account, provider) gateway rate limits: account-wide RPM/TPM ceilings a
-- pay-per-token provider (e.g. Fireworks) shares across every concurrent
-- session. Stored as a validated JSONB `{ "rpm": int?, "tpm": int? }`; NULL or
-- an empty object ⇒ no limiting (prior behaviour). Enforced in the gateway
-- proxy path against in-memory per-provider sliding windows.
ALTER TABLE account_providers ADD COLUMN rate_limits_json JSONB;
