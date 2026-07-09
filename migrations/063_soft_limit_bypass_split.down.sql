-- CCT-484 down: collapse the per-window bypasses back into one column. Lossy
-- when the two differ; keep the 5h value (the pre-split default use case) and
-- fall back to the 7d one when only it is set.

ALTER TABLE account_providers ADD COLUMN soft_limit_bypass_minutes INT;

UPDATE account_providers
SET soft_limit_bypass_minutes =
    COALESCE(soft_limit_bypass_5h_minutes, soft_limit_bypass_7d_minutes);

ALTER TABLE account_providers
    DROP COLUMN soft_limit_bypass_5h_minutes,
    DROP COLUMN soft_limit_bypass_7d_minutes;
