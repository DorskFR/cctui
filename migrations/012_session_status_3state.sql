-- Collapse the old 5-state session status enum into the 3-state model:
--   registering -> new
--   active      -> active (unchanged)
--   idle        -> active   (idle no longer exists; promote to active so
--                           it can still be demoted by the reaper based on
--                           last_heartbeat — no orphaned state)
--   disconnected, terminated -> inactive
UPDATE sessions SET status = 'new'      WHERE status = 'registering';
UPDATE sessions SET status = 'active'   WHERE status = 'idle';
UPDATE sessions SET status = 'inactive' WHERE status IN ('disconnected', 'terminated');
