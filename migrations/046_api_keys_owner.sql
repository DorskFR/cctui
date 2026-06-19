-- CCT-418: lock down shared resources. Give `api_keys` an owner so the `/keys`
-- routes can scope list/create/delete/get-value to owner-or-admin instead of
-- exposing every stored provider key to any authenticated caller.
--
-- Nullable + ON DELETE SET NULL: pre-existing rows have NULL owner (legacy /
-- admin-owned) and are treated as admin-only-visible by the route layer; if the
-- owning user is later purged the key survives as an admin-owned orphan rather
-- than cascading away a still-valid provider secret.
ALTER TABLE api_keys
    ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS api_keys_user_id_idx ON api_keys (user_id);
