-- The memories a session was actually delivered, at the version it received.
ALTER TABLE session_context
    ADD COLUMN IF NOT EXISTS memories JSONB NOT NULL DEFAULT '[]'::jsonb;
