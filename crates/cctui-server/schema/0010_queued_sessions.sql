ALTER TABLE sessions DROP CONSTRAINT IF EXISTS sessions_status_check;

ALTER TABLE sessions
    ADD CONSTRAINT sessions_status_check
    CHECK (status IN ('new', 'active', 'inactive', 'archived', 'draft', 'ended', 'failed', 'queued'));

CREATE INDEX inbox_lanes_session ON inbox_lanes (session_id) WHERE session_id IS NOT NULL;
