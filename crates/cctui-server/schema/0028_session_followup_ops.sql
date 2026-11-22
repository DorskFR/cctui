CREATE TABLE session_followup_ops (
    id                UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    source_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    owner_id          UUID,
    idempotency_key   TEXT NOT NULL,
    reason            TEXT NOT NULL CHECK (reason IN ('cold', 'manual', 'auto', 'inbox')),
    instruction       TEXT NOT NULL DEFAULT '',
    archive_source    BOOLEAN NOT NULL DEFAULT true,
    command_id        UUID NOT NULL,
    spawn_key         TEXT,
    target_session_id TEXT,
    state             TEXT NOT NULL DEFAULT 'pending'
                      CHECK (state IN ('pending', 'registered', 'failed')),
    error             TEXT,
    archive_error     TEXT,
    archived          BOOLEAN NOT NULL DEFAULT false,
    registered_at     TIMESTAMPTZ,
    settled_at        TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (source_session_id, idempotency_key)
);

CREATE UNIQUE INDEX session_followup_ops_one_pending
    ON session_followup_ops (source_session_id) WHERE state = 'pending';

CREATE INDEX session_followup_ops_command ON session_followup_ops (command_id);

CREATE INDEX session_followup_ops_unsettled ON session_followup_ops (created_at)
    WHERE settled_at IS NULL;
