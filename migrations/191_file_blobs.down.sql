-- Copy file-backed bytes into PostgreSQL before rolling back.
DO $$ BEGIN
IF EXISTS (SELECT 1 FROM daemon_blobs WHERE file_stored) THEN
RAISE EXCEPTION 'Restore file-backed blobs before rollback';
END IF;
END $$;
ALTER TABLE daemon_blobs DROP COLUMN file_stored;
