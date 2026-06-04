-- CCT-222: operator-set color override per machine. NULL = derive from the
-- name hash client-side (legacy behavior). Stored as an HSL hue (0-359).
ALTER TABLE machines
    ADD COLUMN hue SMALLINT CHECK (hue >= 0 AND hue < 360);
