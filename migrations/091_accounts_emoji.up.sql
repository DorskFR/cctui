-- An account's identity glyph. NULL ⇒ the UI draws a colour square derived from
-- the account id with the first letter of the name, so every account has a mark
-- whether or not its owner picked one.
ALTER TABLE accounts ADD COLUMN IF NOT EXISTS emoji TEXT;
