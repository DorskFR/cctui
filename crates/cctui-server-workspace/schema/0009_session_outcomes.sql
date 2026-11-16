CREATE TABLE session_outcomes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    dispatch_id UUID,
    turn_seq    BIGINT,
    status      TEXT NOT NULL
                CHECK (status IN ('result', 'needs_input', 'failed', 'ended_without_outcome')),
    headline    TEXT NOT NULL,
    body        TEXT,
    evidence    TEXT[] NOT NULL DEFAULT '{}',
    source      TEXT NOT NULL CHECK (source IN ('tool', 'result_file', 'text', 'inferred')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX session_outcomes_session_idx ON session_outcomes (session_id, created_at DESC);
