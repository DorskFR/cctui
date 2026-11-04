-- The real text of a message carrying <redacted> spans, encrypted with the vault
-- key; `body` then holds the masked form shown in listings.
ALTER TABLE session_message_queue ADD COLUMN IF NOT EXISTS sealed_body TEXT;
