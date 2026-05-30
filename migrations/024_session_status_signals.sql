-- CCT-124: persist the classifier signals the daemon already emits in
-- `AdapterEvent::Status` (tempo / state / activity) so the server can derive
-- the "needs input" attention flag for the sessions list. Previously Status
-- events were dropped on the floor (only a heartbeat bump), so the richer
-- signal never reached the UI.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS tempo TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS agent_state TEXT;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS activity TEXT;
