-- What each dispatch asked for, so a worker that never registers can be
-- identified and re-run. Never holds env values or callback secrets.
CREATE TABLE IF NOT EXISTS dispatch_records (
    session_id       TEXT PRIMARY KEY,
    user_id          UUID REFERENCES users(id) ON DELETE CASCADE,
    dispatcher_name  TEXT NOT NULL,
    name             TEXT,
    profile          TEXT,
    repo             TEXT,
    model            TEXT,
    adapter          TEXT,
    prompt_head      TEXT,
    prompt_file      TEXT,
    prompt_truncated BOOLEAN NOT NULL DEFAULT false,
    env_keys         TEXT[] NOT NULL DEFAULT '{}',
    -- The request to re-send, without env, callbacks or the prompt; NULL when
    -- too large to keep.
    replay           JSONB,
    outcome          TEXT NOT NULL DEFAULT 'pending'
                     CHECK (outcome IN ('pending', 'forwarded', 'registered', 'refused', 'failed')),
    end_reason       TEXT,
    detail           TEXT,
    redispatched_as  TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS dispatch_records_by_user_time
    ON dispatch_records (user_id, created_at);
