-- One link binds one local session (or, host side, one room) to one session on
-- another cctui. Each side keeps its own row and its own Ed25519 keypair.
CREATE TABLE IF NOT EXISTS cctuiverse_links (
    id                    UUID PRIMARY KEY,
    user_id               UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    session_id            TEXT REFERENCES sessions(id) ON DELETE CASCADE,
    room_id               UUID REFERENCES rooms(id) ON DELETE CASCADE,
    kind                  TEXT NOT NULL CHECK (kind IN ('session', 'room')),
    role                  TEXT NOT NULL CHECK (role IN ('inviter', 'joiner')),
    state                 TEXT NOT NULL CHECK (state IN ('pending', 'active', 'closed')),
    label                 TEXT NOT NULL,
    peer_label            TEXT,
    peer_room_name        TEXT,
    public_key            BYTEA NOT NULL,
    encrypted_private_key TEXT,
    peer_public_key       BYTEA,
    peer_link_id          UUID,
    peer_url              TEXT,
    invite_token_hash     BYTEA,
    invite_expires_at     TIMESTAMPTZ,
    envelope_nonce        TEXT NOT NULL,
    settings              JSONB NOT NULL DEFAULT '{}'::jsonb,
    sent_count            INTEGER NOT NULL DEFAULT 0,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    activated_at          TIMESTAMPTZ,
    closed_at             TIMESTAMPTZ,
    CHECK ((kind = 'session' AND session_id IS NOT NULL)
        OR (kind = 'room' AND (room_id IS NOT NULL OR session_id IS NOT NULL)))
);

CREATE INDEX IF NOT EXISTS cctuiverse_links_session
    ON cctuiverse_links (session_id) WHERE state <> 'closed';
CREATE INDEX IF NOT EXISTS cctuiverse_links_room
    ON cctuiverse_links (room_id) WHERE state = 'active';
CREATE INDEX IF NOT EXISTS cctuiverse_links_user ON cctuiverse_links (user_id);

-- Both directions: the audit trail, the outbox and the hold queue.
CREATE TABLE IF NOT EXISTS cctuiverse_messages (
    id              BIGSERIAL PRIMARY KEY,
    link_id         UUID NOT NULL REFERENCES cctuiverse_links(id) ON DELETE CASCADE,
    message_id      UUID NOT NULL,
    direction       TEXT NOT NULL CHECK (direction IN ('in', 'out')),
    kind            TEXT NOT NULL CHECK (kind IN ('direct', 'room_post', 'close')),
    body            JSONB NOT NULL,
    status          TEXT NOT NULL CHECK (status IN
                        ('review', 'queued', 'delivered', 'held', 'released', 'dropped', 'failed')),
    attempts        INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ,
    last_error      TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    delivered_at    TIMESTAMPTZ,
    UNIQUE (link_id, direction, message_id)
);

CREATE INDEX IF NOT EXISTS cctuiverse_messages_outbox
    ON cctuiverse_messages (status, next_attempt_at)
    WHERE direction = 'out' AND status = 'queued';

-- Signature nonces seen inside the replay window.
CREATE TABLE IF NOT EXISTS cctuiverse_nonces (
    link_id UUID NOT NULL,
    nonce   TEXT NOT NULL,
    seen_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (link_id, nonce)
);
