-- CCT-296: per-session ephemeral machine keys for dispatched workers.
--
-- Until now every dispatched worker pod of a user authenticated with the same
-- shared per-user `machines.dispatch_key` (CCT-191): one compromised pod could
-- impersonate every dispatched session of that user. Now the dispatch flow
-- mints an EPHEMERAL machine credential PER SESSION at dispatch time and injects
-- THAT as `CCTUI_MACHINE_KEY` (no worker contract change). The blast radius of a
-- leaked worker key shrinks to its own session, and the key expires with it.
--
-- An ephemeral key is just an `auth_keys` row (CCT-410) — the auth path already
-- enforces `expires_at` and carries `machine_id` transparently — with:
--   * kind = 'ephemeral'
--   * machine_id = the user's shared `dispatch` machine (so sessions still group
--     under that one logical machine in the UI — grouping is unchanged)
--   * expires_at = session deadline + grace
--   * session_id = the pre-minted dispatch session id it is bound to
--
-- The shared `dispatch` machine row and `dispatch_key` REMAIN (existing flows
-- and the admin-token path are unchanged); only the per-session credential is
-- additive.

-- Bind an auth key to a single dispatched session. NULL for every non-ephemeral
-- key. Used to revoke the key when its session reaches a terminal state.
ALTER TABLE auth_keys ADD COLUMN IF NOT EXISTS session_id TEXT;

CREATE INDEX IF NOT EXISTS idx_auth_keys_session
    ON auth_keys(session_id) WHERE session_id IS NOT NULL;

-- The reaper sweeps expired ephemeral keys by (kind, expires_at); index it.
CREATE INDEX IF NOT EXISTS idx_auth_keys_ephemeral_expiry
    ON auth_keys(expires_at) WHERE kind = 'ephemeral';
