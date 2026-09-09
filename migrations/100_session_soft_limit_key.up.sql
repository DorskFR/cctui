-- Machine-readable companion to `sessions.soft_limit_reason` (prose, shown in
-- the UI): which window's cap refused the request, and whether that cap came
-- from the account configuration or from the session's own `session_usd`
-- budget. Raising an account cap may only lift the blocks it owns.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS soft_limit_key TEXT;
