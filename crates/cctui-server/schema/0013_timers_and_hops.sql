CREATE TABLE session_timers (
    session_id   TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    timer_at     TIMESTAMPTZ,
    timer_prompt TEXT,
    timer_ticks  INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX session_timers_due ON session_timers (timer_at) WHERE timer_at IS NOT NULL;

ALTER TABLE inbox_events
    ADD CONSTRAINT inbox_events_hops_check CHECK (hops BETWEEN 0 AND 8);
