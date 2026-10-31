UPDATE sessions
SET session_name = metadata->>'custom_title'
WHERE COALESCE(session_name, '') = ''
  AND metadata->>'name_source' = 'user'
  AND NULLIF(btrim(metadata->>'custom_title'), '') IS NOT NULL;
