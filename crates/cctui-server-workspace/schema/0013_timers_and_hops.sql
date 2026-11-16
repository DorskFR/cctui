ALTER TABLE sessions
    ADD COLUMN timer_at     TIMESTAMPTZ,
    ADD COLUMN timer_prompt TEXT,
    ADD COLUMN timer_ticks  INTEGER NOT NULL DEFAULT 0;

CREATE INDEX sessions_timer_due ON sessions (timer_at) WHERE timer_at IS NOT NULL;

ALTER TABLE inbox_events
    ADD CONSTRAINT inbox_events_hops_check CHECK (hops BETWEEN 0 AND 8);
