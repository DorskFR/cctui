CREATE TABLE redteam_runs (
    id          BIGSERIAL PRIMARY KEY,
    tier        TEXT NOT NULL CHECK (tier IN ('t2', 't3')),
    lane        TEXT NOT NULL DEFAULT '',
    version     TEXT NOT NULL CHECK (version ~ '^[0-9A-Za-z.+-]{1,64}$'),
    run_id      TEXT NOT NULL,
    verdict     TEXT NOT NULL,
    finished_at TIMESTAMPTZ NOT NULL,
    received_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    posted_by   UUID,
    report      JSONB NOT NULL CHECK (octet_length(report::text) <= 262144)
);
CREATE INDEX redteam_runs_latest
    ON redteam_runs (version, tier, lane, finished_at DESC, id DESC);
CREATE INDEX redteam_runs_lane ON redteam_runs (tier, lane, id DESC);
