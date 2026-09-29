-- Drafts scheduled to launch later. The spawn payload stays on the draft
-- session row (`metadata.draft`); this table holds only the schedule and the
-- delivery bookkeeping, so a cancelled or discarded draft takes its schedule
-- with it.
CREATE TABLE IF NOT EXISTS draft_launch_queue (
    draft_id        TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    user_id         UUID REFERENCES users(id) ON DELETE CASCADE,
    -- scheduled | launching | launched | dead
    state           TEXT NOT NULL DEFAULT 'scheduled',
    launch_at       TIMESTAMPTZ NOT NULL,
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    launched_at     TIMESTAMPTZ,
    CONSTRAINT draft_launch_queue_state_check
        CHECK (state IN ('scheduled', 'launching', 'launched', 'dead'))
);

CREATE INDEX IF NOT EXISTS idx_draft_launch_queue_due
    ON draft_launch_queue (state, launch_at)
    WHERE state = 'scheduled';
