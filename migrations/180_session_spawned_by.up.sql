-- Who spawned a session, distinct from its owner (the machine's owner): a
-- machine-share grantee spawns sessions the machine owner keeps owning.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS spawned_by UUID REFERENCES users(id) ON DELETE SET NULL;

-- The spawner chosen at spawn time, waiting for the session to register.
-- Keyed by the spawn key like `session_label_intents`.
CREATE TABLE IF NOT EXISTS session_spawner_intents (
  spawn_key  TEXT PRIMARY KEY,
  user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
