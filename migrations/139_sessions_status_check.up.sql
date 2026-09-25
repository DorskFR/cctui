ALTER TABLE sessions
    ADD CONSTRAINT sessions_status_check
    CHECK (status IN ('new', 'active', 'inactive', 'archived', 'draft', 'ended', 'failed'));
