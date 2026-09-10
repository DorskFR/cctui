DROP INDEX IF EXISTS session_profiles_user_order_idx;
ALTER TABLE session_profiles DROP COLUMN IF EXISTS sort_order;
