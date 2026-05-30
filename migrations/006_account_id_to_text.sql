-- account_id was left as UUID when session IDs were converted to TEXT.
-- Convert it too so the server can bind String values.
ALTER TABLE sessions ALTER COLUMN account_id TYPE TEXT USING account_id::TEXT;
