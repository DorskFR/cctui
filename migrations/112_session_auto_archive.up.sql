-- Spawns that asked to be archived once their first turn ends cleanly
-- (macros). Keyed by the spawn key the gateway token is bound to (the
-- pre-minted session id for claude-code, the command id otherwise); the row
-- is consumed when the session registers and its intent moves to
-- `sessions.metadata.auto_archive`. Rows nobody claims are pruned by the reaper.
CREATE TABLE IF NOT EXISTS session_auto_archive (
  spawn_key  TEXT PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
