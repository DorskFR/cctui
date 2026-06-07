-- CCT-251: non-destructive on/off toggle for a user, distinct from revoke.
-- While set, the user's tokens and machine keys fail auth; clearing it
-- restores them unchanged (revoke remains the permanent kill switch).
ALTER TABLE users ADD COLUMN disabled_at TIMESTAMPTZ;
