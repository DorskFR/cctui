-- Rolling back would deliver the masked text in place of the sealed one.
DO $$ BEGIN
IF EXISTS (SELECT 1 FROM session_message_queue
           WHERE sealed_body IS NOT NULL AND state IN ('scheduled', 'sending')) THEN
RAISE EXCEPTION 'Deliver or cancel sealed scheduled messages before rollback';
END IF;
END $$;
ALTER TABLE session_message_queue DROP COLUMN IF EXISTS sealed_body;
