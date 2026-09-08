-- Bind a dispatcher to an account POOL by default, not just to one account.
--
-- `default_account_id` pins one credential, so every dispatch that names no
-- account rode that one account until it hit its limit and started refusing.
-- A pool binding instead elects a member per dispatch, by the pool's strategy.
--
-- ON DELETE SET NULL matches `default_account_id`: deleting a pool unbinds the
-- dispatcher rather than blocking the delete. Both columns may be set; the
-- account is the more specific instruction and wins.

ALTER TABLE dispatchers
    ADD COLUMN IF NOT EXISTS default_pool_id UUID
        REFERENCES account_pools (id) ON DELETE SET NULL;
