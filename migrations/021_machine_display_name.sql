-- 021: optional friendly override for machine name.
-- `machines.name` is set from the enrollment hostname; `display_name`
-- lets the operator rename a machine for the UI without re-enrolling.
ALTER TABLE machines ADD COLUMN IF NOT EXISTS display_name TEXT NULL;
