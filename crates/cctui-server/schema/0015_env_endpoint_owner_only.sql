-- The server's ntfy endpoint belongs to the instance owner alone.
UPDATE notification_endpoints SET enabled = FALSE
WHERE config ->> 'source' = 'env' AND enabled
  AND user_id IS DISTINCT FROM (
      SELECT id FROM users WHERE revoked_at IS NULL AND disabled_at IS NULL
      ORDER BY created_at, id LIMIT 1);
