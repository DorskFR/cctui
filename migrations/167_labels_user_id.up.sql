-- Label definitions were global: every user listed, renamed and deleted every
-- other tenant's labels, and the get-or-create upsert collided on a global
-- lower(name). Labels are per-user vocabulary.
ALTER TABLE labels ADD COLUMN IF NOT EXISTS user_id UUID REFERENCES users (id) ON DELETE CASCADE;
DROP INDEX IF EXISTS labels_name_lower_key;

-- Backfill from the attachments: a label belongs to whoever uses it. The user
-- with the most sessions carrying it keeps the row; every other user that had
-- it attached gets a copy of their own, with their attachments moved onto it,
-- so nobody loses a label they were using.
CREATE OR REPLACE VIEW label_usage_backfill_175 AS
    SELECT sl.label_id,
           COALESCE(s.user_id, m.user_id) AS user_id,
           count(*)                       AS n
      FROM session_labels sl
      JOIN sessions s ON s.id = sl.session_id
      LEFT JOIN machines m ON m.id = s.machine_uuid
     WHERE COALESCE(s.user_id, m.user_id) IS NOT NULL
     GROUP BY 1, 2;

UPDATE labels l SET user_id = w.user_id
  FROM (SELECT DISTINCT ON (label_id) label_id, user_id
          FROM label_usage_backfill_175
         ORDER BY label_id, n DESC, user_id) w
 WHERE w.label_id = l.id;

DO $$
DECLARE
    rec  RECORD;
    copy UUID;
BEGIN
    FOR rec IN
        SELECT u.label_id, u.user_id, l.name, l.color, l.created_at
          FROM label_usage_backfill_175 u
          JOIN labels l ON l.id = u.label_id
         WHERE l.user_id IS DISTINCT FROM u.user_id
    LOOP
        INSERT INTO labels (name, color, created_at, user_id)
        VALUES (rec.name, rec.color, rec.created_at, rec.user_id)
        RETURNING id INTO copy;

        UPDATE session_labels sl SET label_id = copy
          FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid
         WHERE sl.session_id = s.id
           AND sl.label_id = rec.label_id
           AND COALESCE(s.user_id, m.user_id) = rec.user_id;
    END LOOP;
END $$;

DROP VIEW label_usage_backfill_175;

-- Labels nobody ever attached cannot be attributed. On a single-user install
-- they are that user's; on a multi-tenant one they are unused, so they go.
UPDATE labels SET user_id = (SELECT id FROM users LIMIT 1)
 WHERE user_id IS NULL AND (SELECT count(*) FROM users) = 1;
DELETE FROM labels WHERE user_id IS NULL;

ALTER TABLE labels ALTER COLUMN user_id SET NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS labels_user_name_lower_key
    ON labels (user_id, lower(name));
