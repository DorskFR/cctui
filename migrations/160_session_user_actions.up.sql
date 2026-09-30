-- What an agent is waiting on from the human, as opposed to `sessions.todos`,
-- which is the agent's own plan. Server-authoritative: both the agent (over the
-- MCP tools) and the user (in the webui) resolve items here, so neither side has
-- to infer the other's ticks from observed tool calls.
CREATE TABLE session_user_actions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    detail      TEXT,
    kind        TEXT NOT NULL DEFAULT 'action',
    blocking    BOOLEAN NOT NULL DEFAULT FALSE,
    status      TEXT NOT NULL DEFAULT 'open',
    note        TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_at TIMESTAMPTZ,
    resolved_by TEXT
);

CREATE INDEX session_user_actions_session_idx
    ON session_user_actions (session_id, status, created_at);
