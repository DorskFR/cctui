CREATE TABLE inbox_lanes (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    key           TEXT NOT NULL,
    preset_id     UUID,
    session_id    TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    claimed_until TIMESTAMPTZ,
    paused        BOOLEAN NOT NULL DEFAULT false,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, key)
);

CREATE TABLE inbox_events (
    id                   UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    seq                  BIGINT GENERATED ALWAYS AS IDENTITY,
    lane_id              UUID NOT NULL REFERENCES inbox_lanes(id) ON DELETE CASCADE,
    user_id              UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    source               TEXT NOT NULL
                         CHECK (source IN ('n8n', 'webhook', 'schedule', 'timer', 'outcome',
                                           'approval', 'human')),
    source_id            TEXT,
    body                 TEXT NOT NULL,
    sealed_body          TEXT,
    payload              JSONB,
    parent_session_id    TEXT,
    hops                 SMALLINT NOT NULL DEFAULT 0,
    status               TEXT NOT NULL DEFAULT 'pending'
                         CHECK (status IN ('pending', 'claimed', 'delivered', 'cancelled', 'dead')),
    not_before           TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at           TIMESTAMPTZ,
    attempts             INTEGER NOT NULL DEFAULT 0,
    next_attempt_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_error           TEXT,
    batch_id             UUID,
    delivered_session_id TEXT,
    delivered_as         TEXT CHECK (delivered_as IN ('reply', 'spawn', 'followup', 'dispatch')),
    created_at           TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at         TIMESTAMPTZ
);

-- Not partial: NULLs stay distinct, so a NULL source_id never dedups, and
-- `ON CONFLICT (lane_id, source, source_id)` can infer this index.
CREATE UNIQUE INDEX inbox_events_source ON inbox_events (lane_id, source, source_id);
CREATE INDEX inbox_events_due ON inbox_events (lane_id, seq) WHERE status = 'pending';
CREATE INDEX inbox_events_lease ON inbox_events (next_attempt_at) WHERE status = 'claimed';
CREATE INDEX inbox_events_prune ON inbox_events (delivered_at) WHERE status = 'delivered';
CREATE INDEX inbox_events_user ON inbox_events (user_id, created_at DESC);
