-- The name requested at spawn, pinned onto the session when it registers.
ALTER TABLE session_spawner_intents ADD COLUMN IF NOT EXISTS session_name TEXT;
