-- A privacy scan runs for minutes; its progress polls and its cancel can land
-- on any replica, so the job lives in the database rather than in the memory of
-- whichever pod started it.
CREATE TABLE IF NOT EXISTS privacy_scan_jobs (
    id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    dry_run boolean NOT NULL,
    session_ids text[],
    since timestamptz,
    status text NOT NULL,
    cancel_requested boolean NOT NULL DEFAULT false,
    rows_total bigint,
    rows_scanned bigint NOT NULL DEFAULT 0,
    rows_changed bigint NOT NULL DEFAULT 0,
    substitutions bigint NOT NULL DEFAULT 0,
    by_category jsonb NOT NULL DEFAULT '{}'::jsonb,
    samples jsonb NOT NULL DEFAULT '[]'::jsonb,
    error text,
    created_at timestamptz NOT NULL DEFAULT now(),
    -- Heartbeat: a 'running' row whose worker died with its pod is reaped on the
    -- next start instead of wedging the user's one-at-a-time slot forever.
    updated_at timestamptz NOT NULL DEFAULT now(),
    finished_at timestamptz,
    CONSTRAINT privacy_scan_jobs_status_check
    CHECK (status IN ('running', 'completed', 'cancelled', 'failed'))
);

CREATE UNIQUE INDEX IF NOT EXISTS privacy_scan_jobs_one_running_idx
ON privacy_scan_jobs (user_id) WHERE status = 'running';

CREATE INDEX IF NOT EXISTS privacy_scan_jobs_user_created_idx
ON privacy_scan_jobs (user_id, created_at DESC);
