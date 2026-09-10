-- User-controlled profile order for the spawn panel. Backfilled from the order
-- the list endpoint used before (created_at, lower(name)) so nothing moves on
-- deploy; new profiles take MAX(sort_order) + 1 and land at the end.
ALTER TABLE session_profiles ADD COLUMN IF NOT EXISTS sort_order INTEGER NOT NULL DEFAULT 0;

UPDATE session_profiles p
   SET sort_order = ranked.rn
  FROM (
        SELECT id,
               (row_number() OVER (PARTITION BY user_id ORDER BY created_at, lower(name)))::int - 1
                   AS rn
          FROM session_profiles
       ) ranked
 WHERE p.id = ranked.id;

CREATE INDEX IF NOT EXISTS session_profiles_user_order_idx
    ON session_profiles (user_id, sort_order);
