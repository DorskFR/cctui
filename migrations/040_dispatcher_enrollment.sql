-- CCT-285 (finishing CCT-248): rework the `dispatchers` table from
-- server-instantiated executor *config* blobs (CCT-235) into enrolled-dispatcher
-- *identity* records — peers of `machines`. A row no longer carries an
-- in-process kube/docker config the server materializes; it carries the
-- enrollment credential of a standalone executor service that dials out over
-- `/api/v1/dispatcher/ws` and receives key-checked Dispatch commands.
--
-- Forward-only and idempotent: add identity columns, drop the legacy `config`
-- blob, and clear any pre-existing CCT-235 config-blob rows (they have no
-- enrollment key and can never authenticate under the new model). The table
-- name is preserved so existing references stay valid.

-- Enrollment credential + identity columns (mirrors `machines`).
ALTER TABLE dispatchers ADD COLUMN IF NOT EXISTS key_hash     TEXT;
ALTER TABLE dispatchers ADD COLUMN IF NOT EXISTS key_preview  TEXT;
ALTER TABLE dispatchers ADD COLUMN IF NOT EXISTS last_seen_at TIMESTAMPTZ NOT NULL DEFAULT now();
ALTER TABLE dispatchers ADD COLUMN IF NOT EXISTS revoked_at   TIMESTAMPTZ;

-- Old CCT-235 rows carried a `config` blob and no enrollment key. They cannot
-- authenticate as enrolled dispatchers, so retire them (soft-delete) before we
-- drop the column. New rows are created by the enroll route with a key.
UPDATE dispatchers
    SET deleted_at = COALESCE(deleted_at, now())
    WHERE key_hash IS NULL AND deleted_at IS NULL;

ALTER TABLE dispatchers DROP COLUMN IF EXISTS config;

-- Enrollment keys are unique across all dispatchers (the auth path resolves a
-- token hash to exactly one dispatcher identity).
CREATE UNIQUE INDEX IF NOT EXISTS dispatchers_key_hash_uniq
    ON dispatchers (key_hash)
    WHERE key_hash IS NOT NULL;
