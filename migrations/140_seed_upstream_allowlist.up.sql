-- Upstream hosts become an admin setting that wins over the env seed, so every
-- base_url already stored keeps working after the upgrade.
WITH parsed AS (
    SELECT lower(scheme) AS scheme,
           rtrim(lower(host), '.') AS host,
           port
      FROM account_providers ap,
           regexp_match(
               btrim(ap.base_url),
               '^([A-Za-z][A-Za-z0-9+.-]*)://(?:[^@/?#]*@)?(\[[^\]]+\]|[^:/?#]+)(?::([0-9]+))?'
           ) AS m(parts),
           LATERAL (SELECT m.parts[1] AS scheme, m.parts[2] AS host, m.parts[3] AS port) p
     WHERE ap.base_url IS NOT NULL AND NOT ap.managed
),
hosts AS (
    SELECT DISTINCT host || ':' || COALESCE(
               NULLIF(ltrim(port, '0'), ''),
               CASE scheme WHEN 'http' THEN '80' WHEN 'https' THEN '443' END
           ) AS entry
      FROM parsed
     WHERE scheme IN ('http', 'https') AND host <> ''
)
INSERT INTO instance_settings (key, value, updated_at)
SELECT 'upstream_allowed_hosts', jsonb_agg(entry ORDER BY entry), now()
  FROM hosts
HAVING count(*) > 0
ON CONFLICT (key) DO UPDATE
   SET value = (
           SELECT jsonb_agg(DISTINCT e ORDER BY e)
             FROM jsonb_array_elements(instance_settings.value || EXCLUDED.value) AS t(e)
       ),
       updated_at = now();
