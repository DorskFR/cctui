-- CCT-484: split the single soft-limit bypass into per-window bypasses. The 5h
-- window suits a short bypass (~15-30 min); the 7d window needs hours to ever
-- fire. Backfill both from the old shared value, then drop it — nothing reads
-- soft_limit_bypass_minutes after this release.

ALTER TABLE account_providers
    ADD COLUMN soft_limit_bypass_5h_minutes INT,
    ADD COLUMN soft_limit_bypass_7d_minutes INT;

UPDATE account_providers
SET soft_limit_bypass_5h_minutes = soft_limit_bypass_minutes,
    soft_limit_bypass_7d_minutes = soft_limit_bypass_minutes;

ALTER TABLE account_providers DROP COLUMN soft_limit_bypass_minutes;
