UPDATE sessions
SET model = NULLIF(metadata->>'model', '')
WHERE model IS NULL
  AND NULLIF(metadata->>'model', '') IS NOT NULL;

UPDATE sessions AS s
SET machine_uuid = (
    SELECT m.id
    FROM machines AS m
    WHERE m.revoked_at IS NULL
      AND (m.id::text = s.machine_id OR m.name = s.machine_id)
      AND (s.user_id IS NULL OR m.user_id = s.user_id)
)
WHERE s.machine_uuid IS NULL
  AND 1 = (
      SELECT count(*)
      FROM machines AS m
      WHERE m.revoked_at IS NULL
        AND (m.id::text = s.machine_id OR m.name = s.machine_id)
        AND (s.user_id IS NULL OR m.user_id = s.user_id)
  );
