-- A room is two things and nothing more: a field on `sessions` that groups them
-- in the UI, and a permission boundary letting its sessions address each other
-- (see `peer_policy`). There is no membership table — a session belongs to at
-- most one room, so the room IS a column, and moving a session is one UPDATE.
CREATE TABLE IF NOT EXISTS rooms (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    next_seq    BIGINT NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ
);

-- Rooms are created implicitly by name, so the name is the identity a caller
-- has: two rooms of one owner cannot share it.
CREATE UNIQUE INDEX IF NOT EXISTS rooms_owner_name ON rooms (user_id, lower(name));
CREATE INDEX IF NOT EXISTS rooms_user_live ON rooms (user_id) WHERE archived_at IS NULL;

-- `ON DELETE SET NULL`: deleting a room releases its sessions rather than
-- taking them with it.
ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS room_id UUID REFERENCES rooms(id) ON DELETE SET NULL;

-- The broadcast delivery cursor, per session because a session is in one room.
ALTER TABLE sessions
    ADD COLUMN IF NOT EXISTS room_delivered_seq BIGINT NOT NULL DEFAULT 0;

CREATE INDEX IF NOT EXISTS sessions_room ON sessions (room_id) WHERE room_id IS NOT NULL;

-- `sender_label` is denormalized on purpose: a sender that is later archived or
-- deleted must not blank out its past posts.
CREATE TABLE IF NOT EXISTS room_messages (
    id                BIGSERIAL PRIMARY KEY,
    room_id           UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    seq               BIGINT NOT NULL,
    sender_session_id TEXT REFERENCES sessions(id) ON DELETE SET NULL,
    sender_label      TEXT NOT NULL,
    body              TEXT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (room_id, seq)
);

CREATE INDEX IF NOT EXISTS room_messages_room_seq ON room_messages (room_id, seq);
