DROP TABLE IF EXISTS self_update_runs;
ALTER TABLE machines DROP COLUMN IF EXISTS update_hook;
