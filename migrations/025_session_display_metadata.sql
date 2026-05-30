-- CCT-113: persist per-session display metadata the daemon emits via
-- AdapterEvent::Status (user-defined name, model, reasoning effort) so the
-- sessions list can show what each session is and how it's configured.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS session_name TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS model TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS effort TEXT;
