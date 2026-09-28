-- Re-key the gateway tokens of codex sessions spawned before the server handed
-- the daemon its dispatch key.
--
-- A codex spawn binds its token to the dispatch `command_id` because codex
-- mints its own thread id. The server never sent that key to the daemon, so
-- the thread registered with `spawn_key = null` and the token stayed on an id
-- no session has: `account_name` null, no switch-account, no revocation at
-- session end, no metering (the usage FK targets `sessions(id)`).
--
-- Nothing persisted links a dispatch key to its thread, so the pair is
-- rebuilt from timing: the thread registers within a second of its token
-- being minted, for the same user. A pair is kept only when each side is the
-- other's nearest match within 10 seconds, and only for a codex session that
-- holds no token of its own. Rows that do not pair stay as they are.
--
-- Tokens are not revoked here even when the session has ended: a resumed
-- session can still carry an old `ended_at`, and revoking its token would cut
-- a running agent off the gateway. Orphaned tokens carry an expiry.

CREATE TEMP TABLE codex_token_rebind ON COMMIT DROP AS
WITH orphan AS (
    SELECT st.session_id AS spawn_key,
           min(st.created_at) AS minted_at,
           min(ap.user_id::text)::uuid AS user_id
    FROM session_tokens st
    JOIN account_providers ap ON ap.id = st.account_id
    WHERE NOT EXISTS (SELECT 1 FROM sessions s WHERE s.id = st.session_id)
    GROUP BY st.session_id
    HAVING count(DISTINCT ap.user_id) = 1
),
cand AS (
    SELECT o.spawn_key, s.id AS session_id, s.registered_at - o.minted_at AS lag
    FROM orphan o
    JOIN sessions s
      ON s.adapter_id = 'codex'
     AND s.user_id = o.user_id
     AND s.registered_at >= o.minted_at
     AND s.registered_at < o.minted_at + interval '10 seconds'
    WHERE NOT EXISTS (SELECT 1 FROM session_tokens t WHERE t.session_id = s.id)
),
ranked AS (
    SELECT spawn_key, session_id,
           row_number() OVER (PARTITION BY spawn_key ORDER BY lag, session_id) AS by_key,
           row_number() OVER (PARTITION BY session_id ORDER BY lag, spawn_key) AS by_session
    FROM cand
)
SELECT spawn_key, session_id FROM ranked WHERE by_key = 1 AND by_session = 1;

UPDATE session_tokens st
SET session_id = r.session_id
FROM codex_token_rebind r
WHERE st.session_id = r.spawn_key;

UPDATE session_attachments a
SET session_id = r.session_id
FROM codex_token_rebind r
WHERE a.session_id = r.spawn_key;

UPDATE session_spawn_capabilities c
SET session_id = r.session_id
FROM codex_token_rebind r
WHERE c.session_id = r.spawn_key
  AND NOT EXISTS (
      SELECT 1 FROM session_spawn_capabilities x WHERE x.session_id = r.session_id
  );
