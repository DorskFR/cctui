-- Declarative per-machine adapter configuration. The daemon polls this on
-- connect via the `Reconcile` frame to decide which adapters to instantiate
-- and with what configuration. Out-of-band edits trigger a fresh Reconcile.

CREATE TABLE IF NOT EXISTS adapters_enabled (
    machine_id  UUID NOT NULL REFERENCES machines(id) ON DELETE CASCADE,
    adapter_id  TEXT NOT NULL,
    config      JSONB NOT NULL DEFAULT '{}'::jsonb,
    enabled     BOOLEAN NOT NULL DEFAULT TRUE,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (machine_id, adapter_id)
);
