-- Durable `CctuiAgent` spawn capabilities.
--
-- The capability is declared once by the spawn/dispatch that launches a session
-- and re-served on every launch env pull. Held only in server memory it did not
-- survive a restart, and the fail-closed miss that followed was indistinguishable
-- from "this session may not spawn": the daemon simply stopped offering the tool
-- and the agent recorded a missing sub-agent.
--
-- No FK to `sessions`: the capability is written before the worker registers, so
-- the session row does not exist yet.
CREATE TABLE IF NOT EXISTS session_spawn_capabilities (
    session_id text PRIMARY KEY,
    capability jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
