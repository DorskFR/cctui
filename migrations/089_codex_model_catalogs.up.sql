-- Latest codex `model/list` catalog reported by each machine's daemon, so the
-- model pickers keep the live list across server restarts and the merged
-- cross-machine view can pick the newest report per model id.
CREATE TABLE codex_model_catalogs (
    machine_id  UUID        PRIMARY KEY REFERENCES machines(id) ON DELETE CASCADE,
    catalog     JSONB       NOT NULL,
    fetched_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
