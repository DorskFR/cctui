-- Pin/star sessions (CCT-267): pinned sessions sort above everything in the
-- live list and are exempt from the auto-archive reaper regardless of
-- heartbeat age.
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS pinned BOOLEAN NOT NULL DEFAULT false;
ALTER TABLE sessions ADD COLUMN IF NOT EXISTS pinned_at TIMESTAMPTZ;
