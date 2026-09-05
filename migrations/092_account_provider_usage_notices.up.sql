-- Per-provider usage-notice ticker setting, next to soft_limits_json:
-- `{ "enabled": bool, "step_pct": int }`. NULL ⇒ off.
ALTER TABLE account_providers ADD COLUMN usage_notices JSONB;
