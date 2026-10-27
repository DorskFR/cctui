-- How the worker container ended, captured before its pod is reaped.
ALTER TABLE dispatch_records ADD COLUMN IF NOT EXISTS worker_exit JSONB;

-- Failed-dispatch cards each user has dismissed; pruned with their record.
CREATE TABLE IF NOT EXISTS dispatch_record_dismissals (
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id   TEXT NOT NULL REFERENCES dispatch_records(session_id) ON DELETE CASCADE,
    dismissed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, session_id)
);

CREATE INDEX IF NOT EXISTS dispatch_records_by_user_name
    ON dispatch_records (user_id, name, created_at);
