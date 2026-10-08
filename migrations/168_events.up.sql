-- Lifecycle event log: session / machine / system transitions, append-only.
-- `kind` is free text so a new kind is one call site and no migration.
-- Subject columns SET NULL on delete: the row outlives its subject, and
-- `summary` / `detail` are denormalised so it stays readable afterwards.
CREATE TABLE events (
    id          BIGSERIAL   PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    kind        TEXT        NOT NULL,
    severity    TEXT        NOT NULL DEFAULT 'info',
    session_id  TEXT        REFERENCES sessions(id) ON DELETE SET NULL,
    machine_id  UUID        REFERENCES machines(id) ON DELETE SET NULL,
    user_id     UUID        REFERENCES users(id)    ON DELETE SET NULL,
    actor       TEXT        NOT NULL,
    summary     TEXT        NOT NULL,
    detail      JSONB       NOT NULL DEFAULT '{}'::jsonb
);

CREATE INDEX events_occurred_at_idx ON events (occurred_at DESC, id DESC);
CREATE INDEX events_session_idx     ON events (session_id, occurred_at DESC, id DESC);
CREATE INDEX events_machine_idx     ON events (machine_id, occurred_at DESC, id DESC);
CREATE INDEX events_kind_idx        ON events (kind, occurred_at DESC, id DESC);
