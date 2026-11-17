CREATE TABLE notifications (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,
    severity        TEXT NOT NULL DEFAULT 'info',
    session_id      TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    machine_id      UUID,
    subject_kind    TEXT,
    subject_id      TEXT,
    collapse_key    TEXT,
    title           TEXT NOT NULL,
    body            TEXT CHECK (char_length(body) <= 280),
    tags            TEXT,
    actions         JSONB NOT NULL DEFAULT '[]',
    state           TEXT NOT NULL DEFAULT 'pending'
                    CHECK (state IN ('pending', 'delivered', 'read', 'acted', 'resolved', 'expired')),
    repeat_count    INT NOT NULL DEFAULT 1,
    decision        TEXT,
    decision_note   TEXT,
    acted_channel   TEXT,
    acted_by        UUID,
    resolved_reason TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    read_at         TIMESTAMPTZ,
    acted_at        TIMESTAMPTZ,
    resolved_at     TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ
);
CREATE UNIQUE INDEX notifications_collapse_open ON notifications (user_id, collapse_key)
    WHERE collapse_key IS NOT NULL AND state IN ('pending', 'delivered', 'read');
CREATE INDEX notifications_user_open ON notifications (user_id, created_at DESC)
    WHERE state IN ('pending', 'delivered', 'read', 'acted');
CREATE INDEX notifications_expiry ON notifications (expires_at)
    WHERE expires_at IS NOT NULL AND state IN ('pending', 'delivered', 'read');
CREATE INDEX notifications_subject ON notifications (subject_kind, subject_id);
CREATE INDEX notifications_terminal ON notifications (resolved_at)
    WHERE state IN ('resolved', 'expired');

CREATE TABLE notification_endpoints (
    id         UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel    TEXT NOT NULL,
    label      TEXT,
    config     JSONB NOT NULL,
    enabled    BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_ok_at TIMESTAMPTZ
);
CREATE UNIQUE INDEX notification_endpoints_env ON notification_endpoints (user_id, channel)
    WHERE config ->> 'source' = 'env';

CREATE TABLE notification_deliveries (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    notification_id UUID NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    channel         TEXT NOT NULL,
    endpoint_id     UUID REFERENCES notification_endpoints(id) ON DELETE CASCADE,
    op              TEXT CHECK (op IN ('send', 'update', 'retract')),
    state           TEXT NOT NULL DEFAULT 'scheduled'
                    CHECK (state IN ('scheduled', 'sent', 'cancelled', 'retracted', 'dead')),
    stage           INT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts        INT NOT NULL DEFAULT 0,
    provider_ref    JSONB,
    last_error      TEXT,
    sent_at         TIMESTAMPTZ,
    UNIQUE NULLS NOT DISTINCT (notification_id, channel, endpoint_id)
);
CREATE INDEX notification_deliveries_due ON notification_deliveries (next_attempt_at)
    WHERE op IS NOT NULL;

CREATE TABLE notification_action_tokens (
    token_hash      BYTEA PRIMARY KEY,
    notification_id UUID NOT NULL REFERENCES notifications(id) ON DELETE CASCADE,
    delivery_id     UUID REFERENCES notification_deliveries(id) ON DELETE CASCADE,
    action          TEXT NOT NULL,
    channel         TEXT NOT NULL,
    expires_at      TIMESTAMPTZ NOT NULL,
    used_at         TIMESTAMPTZ
);
CREATE INDEX notification_action_tokens_notification ON notification_action_tokens (notification_id);
