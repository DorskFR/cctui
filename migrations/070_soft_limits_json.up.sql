-- CCT-688: per-window soft limits as a validated JSONB map keyed by canonical
-- window identity (session | weekly_all | weekly_model:<id>). Anthropic now
-- reports a self-describing `limits` array (5h/session, weekly-all, and one or
-- more per-model weekly caps), so a fixed 5h/7d column pair can no longer hold
-- every window. A JSONB map lets a newly discovered model-scoped limit be
-- configured WITHOUT a migration.
--
-- Each value is `{ "cap_pct": int?, "bypass_minutes": int? }`. The legacy fixed
-- columns are backfilled (5h -> session, 7d -> weekly_all) and LEFT IN PLACE so
-- an un-upgraded rollback still reads them; nothing writes them after this.

ALTER TABLE account_providers ADD COLUMN soft_limits_json JSONB;

UPDATE account_providers
SET soft_limits_json = jsonb_strip_nulls(jsonb_build_object(
    'session', CASE
        WHEN soft_limit_5h_pct IS NOT NULL OR soft_limit_bypass_5h_minutes IS NOT NULL
        THEN jsonb_build_object(
            'cap_pct', soft_limit_5h_pct,
            'bypass_minutes', soft_limit_bypass_5h_minutes)
        END,
    'weekly_all', CASE
        WHEN soft_limit_7d_pct IS NOT NULL OR soft_limit_bypass_7d_minutes IS NOT NULL
        THEN jsonb_build_object(
            'cap_pct', soft_limit_7d_pct,
            'bypass_minutes', soft_limit_bypass_7d_minutes)
        END
))
WHERE soft_limit_5h_pct IS NOT NULL
   OR soft_limit_7d_pct IS NOT NULL
   OR soft_limit_bypass_5h_minutes IS NOT NULL
   OR soft_limit_bypass_7d_minutes IS NOT NULL;
