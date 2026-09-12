-- Required by the session-scoped trigram index in 108: btree_gin supplies the
-- GIN opclass for session_id so it can lead a composite GIN key.
CREATE EXTENSION IF NOT EXISTS btree_gin;
