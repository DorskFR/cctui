-- CCT-185: per-user "can dispatch" permission. Gates POST /sessions/dispatch
-- for any caller that owns a user (User token or Machine key). Defaults TRUE so
-- existing behaviour (everyone may dispatch) is preserved until an admin toggles
-- it off; admin/agent (env) tokens have no owning user and bypass the check.
ALTER TABLE users ADD COLUMN IF NOT EXISTS can_dispatch BOOLEAN NOT NULL DEFAULT TRUE;
