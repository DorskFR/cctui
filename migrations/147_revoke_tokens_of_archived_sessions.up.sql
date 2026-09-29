-- Revoke the gateway tokens left live on sessions that are already over.
--
-- Until this release only a session *end* reported by the daemon revoked a
-- session's tokens; archiving one (by hand, in batch, or by the stale sweep)
-- left its token live. A live token still counts as load on its account in
-- `in_flight_by_provider` for the whole recent-binding window, and the rows
-- pile up: an account whose sessions are archived rather than ended looks
-- permanently busy and every election walks away from it, even when it is the
-- freest of the family.
--
-- Only sessions in a terminal state are touched, and only when their token has
-- not served the gateway for 30 minutes: a session archived while its worker
-- kept running (daemon offline when `Remove` was dispatched) would otherwise be
-- cut off mid-inference. The row survives revoked and still names the account,
-- so a resume re-mints a fresh token (`resolve_session_accounts`).

UPDATE session_tokens st
   SET revoked_at = now()
  FROM sessions s
 WHERE s.id = st.session_id
   AND st.revoked_at IS NULL
   AND s.status IN ('archived', 'ended', 'failed')
   AND (st.last_used_at IS NULL OR st.last_used_at < now() - interval '30 minutes');
