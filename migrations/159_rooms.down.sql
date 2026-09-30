DROP INDEX IF EXISTS sessions_room;
ALTER TABLE sessions DROP COLUMN IF EXISTS room_delivered_seq;
ALTER TABLE sessions DROP COLUMN IF EXISTS room_id;
DROP TABLE IF EXISTS room_messages;
DROP TABLE IF EXISTS rooms;
