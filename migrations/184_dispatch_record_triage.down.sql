DROP INDEX IF EXISTS dispatch_records_by_user_name;
DROP TABLE IF EXISTS dispatch_record_dismissals;
ALTER TABLE dispatch_records DROP COLUMN IF EXISTS worker_exit;
