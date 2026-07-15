-- CCT-596: persist `AdapterEvent::Status.intent` — what the session was
-- launched to do. The daemon already emits it (control.rs), but the ingest
-- `Status` destructure dropped it via `..`, so the signal never reached the
-- session row. Stored alongside the other classifier signals (tempo /
-- agent_state / activity) added in 024 so `list_sessions` can surface it as a
-- secondary line / tooltip on the card.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS intent TEXT;
