-- CCT-191: one stable per-user "dispatch" machine identity for dispatched
-- (k8s worker) sessions, replacing the enroll-a-new-ephemeral-machine-per-pod
-- dance. The server lazily creates a single machine of kind 'dispatch' per
-- user, stores its key so it can be RE-USED across dispatches (every worker
-- pod gets the same key and registers its sessions under this one machine),
-- and injects it into the dispatch payload. `dispatch_key` holds the plaintext
-- machine key: it is a bearer credential the server must hand to pods verbatim
-- (it ends up in pod env regardless), so unlike normal machines — where only
-- the hash is kept — the dispatch machine also stores the recoverable key.
-- Scoped to kind = 'dispatch' rows only.
ALTER TABLE machines ADD COLUMN IF NOT EXISTS dispatch_key TEXT;

-- At most one live dispatch machine per user.
CREATE UNIQUE INDEX IF NOT EXISTS uniq_machines_dispatch_per_user
    ON machines(user_id)
    WHERE kind = 'dispatch' AND deleted_at IS NULL;
