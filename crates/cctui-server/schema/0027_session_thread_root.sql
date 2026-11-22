ALTER TABLE sessions ADD COLUMN thread_root_id TEXT;

WITH RECURSIVE up AS (
    SELECT s.id AS member, s.id, s.parent_id, s.metadata->>'relation' AS relation,
           ARRAY[s.id] AS seen
      FROM sessions s
     WHERE s.metadata->>'relation' = 'followup'
    UNION ALL
    SELECT up.member, p.id, p.parent_id, p.metadata->>'relation', up.seen || p.id
      FROM up JOIN sessions p ON p.id = up.parent_id
     WHERE up.relation = 'followup' AND NOT p.id = ANY(up.seen)
       AND cardinality(up.seen) < 500
), roots AS (
    SELECT DISTINCT ON (member) member, id AS root
      FROM up ORDER BY member, cardinality(seen) DESC
), lines AS (
    SELECT member AS id, root FROM roots
    UNION
    SELECT root, root FROM roots
)
UPDATE sessions s SET thread_root_id = lines.root FROM lines WHERE s.id = lines.id;

CREATE INDEX idx_sessions_thread_root_id ON sessions (thread_root_id)
    WHERE thread_root_id IS NOT NULL;
