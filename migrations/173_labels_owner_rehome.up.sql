-- A session's labels live in its owner's vocabulary. Since labels became
-- per-user (167), the attach route let an admin put its own label rows on a
-- user's session, while the spawn path silently dropped them. Move every such
-- cross-owner attachment onto the owner's label of the same name, creating it
-- (same name and color) when the owner has none.
CREATE OR REPLACE VIEW label_cross_owner_173 AS
    SELECT sl.session_id, sl.label_id, sl.created_at, l.name, l.color,
           COALESCE(s.user_id, m.user_id) AS owner
      FROM session_labels sl
      JOIN labels l ON l.id = sl.label_id
      JOIN sessions s ON s.id = sl.session_id
      LEFT JOIN machines m ON m.id = s.machine_uuid
     WHERE COALESCE(s.user_id, m.user_id) IS NOT NULL
       AND l.user_id <> COALESCE(s.user_id, m.user_id);

INSERT INTO labels (name, color, user_id)
SELECT DISTINCT ON (owner, lower(name)) name, color, owner
  FROM label_cross_owner_173
 ORDER BY owner, lower(name), created_at
ON CONFLICT (user_id, lower(name)) DO NOTHING;

INSERT INTO session_labels (session_id, label_id, created_at)
SELECT x.session_id, own.id, x.created_at
  FROM label_cross_owner_173 x
  JOIN labels own ON own.user_id = x.owner AND lower(own.name) = lower(x.name)
ON CONFLICT DO NOTHING;

DELETE FROM session_labels sl
 USING label_cross_owner_173 x
 WHERE sl.session_id = x.session_id AND sl.label_id = x.label_id;

DROP VIEW label_cross_owner_173;
