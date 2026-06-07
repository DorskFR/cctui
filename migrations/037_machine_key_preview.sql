-- CCT-251: non-secret machine-key fragment for display in the admin UI,
-- mirroring user_tokens.token_preview (CCT-185). Set on enroll/rotate;
-- machines enrolled before this column stay NULL (hashes can't be reversed)
-- and render masked.
ALTER TABLE machines ADD COLUMN key_preview TEXT;
