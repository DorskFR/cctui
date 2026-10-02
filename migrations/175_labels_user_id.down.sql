DROP INDEX IF EXISTS labels_user_name_lower_key;
ALTER TABLE labels DROP COLUMN IF EXISTS user_id;

-- Per-user copies made by the up migration collide on the global name index,
-- so the oldest row of each name wins and the rest are folded back onto it.
UPDATE session_labels sl SET label_id = keep.id
  FROM labels l
  JOIN (SELECT DISTINCT ON (lower(name)) lower(name) AS lname, id
          FROM labels ORDER BY lower(name), created_at, id) keep
    ON keep.lname = lower(l.name)
 WHERE sl.label_id = l.id
   AND l.id <> keep.id
   AND NOT EXISTS (SELECT 1 FROM session_labels x
                    WHERE x.session_id = sl.session_id AND x.label_id = keep.id);

DELETE FROM labels l
 WHERE EXISTS (SELECT 1 FROM labels o
                WHERE lower(o.name) = lower(l.name)
                  AND (o.created_at, o.id) < (l.created_at, l.id));

CREATE UNIQUE INDEX IF NOT EXISTS labels_name_lower_key ON labels (lower(name));
