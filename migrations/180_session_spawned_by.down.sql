DROP TABLE IF EXISTS session_spawner_intents;
ALTER TABLE sessions DROP COLUMN IF EXISTS spawned_by;
