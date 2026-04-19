-- Soft-delete tombstone for machines. Admin can hard-remove a revoked row
-- from the UI without breaking FK references from historical sessions /
-- archive entries that still point at the machine's UUID.
ALTER TABLE machines ADD COLUMN IF NOT EXISTS deleted_at TIMESTAMPTZ;
CREATE INDEX IF NOT EXISTS idx_machines_deleted_at ON machines(deleted_at);
