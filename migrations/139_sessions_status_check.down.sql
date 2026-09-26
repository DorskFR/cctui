ALTER TABLE sessions DROP CONSTRAINT IF EXISTS sessions_status_check;

ALTER TABLE sessions ALTER COLUMN status SET DEFAULT 'registering';
