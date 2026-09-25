ALTER TABLE sessions ALTER COLUMN status SET DEFAULT 'new';

ALTER TABLE sessions
    ADD CONSTRAINT sessions_status_check
    CHECK (status IN ('new', 'active', 'inactive', 'archived', 'draft', 'ended', 'failed'));
