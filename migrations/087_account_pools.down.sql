DROP TABLE IF EXISTS session_account_rebinds;
ALTER TABLE session_tokens DROP COLUMN IF EXISTS pool_id;
ALTER TABLE accounts DROP COLUMN IF EXISTS pool_eligible;
DROP TABLE IF EXISTS account_pool_members;
DROP TABLE IF EXISTS account_pools;
