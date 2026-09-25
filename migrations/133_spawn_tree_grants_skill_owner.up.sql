-- Append-only ledger of budgets granted to CctuiAgent children, keyed by the
-- spawn tree's root, so the aggregate stays bounded after children end.
CREATE TABLE IF NOT EXISTS spawn_tree_grants (
    root_id text NOT NULL,
    child_id text PRIMARY KEY,
    budget_usd double precision NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS spawn_tree_grants_root_idx ON spawn_tree_grants (root_id);
