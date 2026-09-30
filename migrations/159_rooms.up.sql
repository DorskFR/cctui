-- Rooms: a named group of sessions (plus the human) with one ordered timeline.
--
-- `rooms.next_seq` is the per-room message counter, bumped under the room's row
-- lock at post time. It is what makes `room_members.last_delivered_seq` a
-- per-room cursor, so an offline member replays exactly the posts it missed, in
-- order, and a redelivery can never duplicate one.
CREATE TABLE IF NOT EXISTS rooms (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name        TEXT NOT NULL,
    next_seq    BIGINT NOT NULL DEFAULT 0,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    archived_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS rooms_user_live ON rooms (user_id) WHERE archived_at IS NULL;

CREATE TABLE IF NOT EXISTS room_members (
    room_id            UUID NOT NULL REFERENCES rooms(id) ON DELETE CASCADE,
    session_id         TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role               TEXT NOT NULL DEFAULT 'member'
                       CHECK (role IN ('member', 'observer')),
    last_delivered_seq BIGINT NOT NULL DEFAULT 0,
    joined_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (room_id, session_id)
);

CREATE INDEX IF NOT EXISTS room_members_session ON room_members (session_id);

-- `sender_label` is denormalized on purpose: a sender that is later archived or
-- deleted must not blank out its past posts in the timeline.
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
