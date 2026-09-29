-- Irreversible on purpose: which tokens were revoked by this backfill is not
-- recorded, and un-revoking every token of an ended session would hand live
-- gateway credentials back to sessions that no longer exist.
SELECT 1;
