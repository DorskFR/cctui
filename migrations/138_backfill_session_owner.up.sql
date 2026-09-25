UPDATE sessions s
   SET user_id = m.user_id
  FROM machines m
 WHERE s.user_id IS NULL
   AND s.machine_uuid = m.id;
