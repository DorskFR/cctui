-- Deterministic update hook (see docs/update-hook.md).
--
-- `machines.update_hook` is advertised by the daemon on every heartbeat, so
-- the server knows whether a machine can update this deployment without an
-- agent. NOT NULL DEFAULT FALSE: a machine that has never said otherwise has
-- no hook.
ALTER TABLE machines ADD COLUMN IF NOT EXISTS update_hook BOOLEAN NOT NULL DEFAULT FALSE;

-- One row per hook run. This table is the run's memory: the update restarts
-- the server, so the process that started a run is almost never the one that
-- records how it ended.
CREATE TABLE IF NOT EXISTS self_update_runs (
    id            UUID PRIMARY KEY,
    machine_id    UUID NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    -- Version asked for, and the one that was running when we asked.
    version       TEXT NOT NULL,
    from_version  TEXT NOT NULL,
    -- `cctui_proto::updatehook::UpdateHookPhase`, in its wire form.
    phase         TEXT NOT NULL,
    exit_code     INTEGER,
    detail        TEXT NOT NULL DEFAULT '',
    output_tail   TEXT,
    -- The admin who clicked. No FK: a run's history should survive the
    -- account that started it being deleted.
    started_by    UUID,
    started_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS self_update_runs_started_at_idx
    ON self_update_runs (started_at DESC);
