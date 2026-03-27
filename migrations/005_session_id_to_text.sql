-- Switch session IDs from UUID to TEXT so we can use Claude's native session ID
-- as the primary key instead of generating our own UUIDs.

-- Drop foreign keys first
ALTER TABLE stream_events DROP CONSTRAINT IF EXISTS stream_events_session_id_fkey;
ALTER TABLE sessions DROP CONSTRAINT IF EXISTS sessions_parent_id_fkey;

-- Convert columns
ALTER TABLE sessions ALTER COLUMN id TYPE TEXT USING id::TEXT;
ALTER TABLE sessions ALTER COLUMN parent_id TYPE TEXT USING parent_id::TEXT;
ALTER TABLE stream_events ALTER COLUMN session_id TYPE TEXT USING session_id::TEXT;

-- Re-add foreign keys
ALTER TABLE stream_events ADD CONSTRAINT stream_events_session_id_fkey
    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE;
ALTER TABLE sessions ADD CONSTRAINT sessions_parent_id_fkey
    FOREIGN KEY (parent_id) REFERENCES sessions(id) ON DELETE SET NULL;
