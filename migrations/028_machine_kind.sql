-- CCT-183: distinguish ephemeral dispatch/worker machines (one pod per
-- dispatched session) from persistent dev-machine daemons, so the web UI can
-- hide them from the New-session machine picker and a reaper can purge orphans
-- left by pods that die before self-deenroll.
-- Values: 'persistent' (default, real daemons) | 'ephemeral' (worker pods).
ALTER TABLE machines ADD COLUMN IF NOT EXISTS kind TEXT NOT NULL DEFAULT 'persistent';

CREATE INDEX IF NOT EXISTS idx_machines_kind ON machines(kind) WHERE kind <> 'persistent';
