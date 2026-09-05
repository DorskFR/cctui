-- Whether this credential's gauge shows in the header strip. Existing rows stay
-- pinned so nothing disappears from a header that already shows them.
ALTER TABLE account_providers ADD COLUMN IF NOT EXISTS header_pin BOOLEAN NOT NULL DEFAULT TRUE;
