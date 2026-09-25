-- Append-only ledger of budgets granted to CctuiAgent children, keyed by the
-- spawn tree's root, so the aggregate stays bounded after children end.
CREATE TABLE IF NOT EXISTS spawn_tree_grants (
    root_id text NOT NULL,
    parent_id text NOT NULL,
    child_id text PRIMARY KEY,
    budget_usd double precision NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS spawn_tree_grants_root_idx ON spawn_tree_grants (root_id);
CREATE INDEX IF NOT EXISTS spawn_tree_grants_parent_idx ON spawn_tree_grants (parent_id);

-- Skills are owned per user: two accounts may publish the same name without
-- touching each other's row. An ownerless row is unreachable (reads filter by
-- owner) and is dropped. Existing bundles move into the owner's directory on
-- first read, when their hash matches the owner's row. Bundles of dropped rows,
-- or never read again, stay as `{name}.tar.zst` at the skill root; nothing
-- serves them and they are safe to delete by hand.
DELETE FROM skill_registry WHERE uploaded_by_user IS NULL;
ALTER TABLE skill_registry DROP CONSTRAINT IF EXISTS skill_registry_pkey;
ALTER TABLE skill_registry ALTER COLUMN uploaded_by_user SET NOT NULL;
ALTER TABLE skill_registry ADD PRIMARY KEY (uploaded_by_user, name);
