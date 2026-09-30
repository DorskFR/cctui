-- Explicit peer-addressing grants between two sessions, the escape hatch of the
-- peer policy for pairs the `parent_id` tree does not relate. Symmetric: one row
-- authorises both directions, because a channel only one side may open is not a
-- channel.
CREATE TABLE IF NOT EXISTS session_peer_shares (
    id           BIGSERIAL PRIMARY KEY,
    session_id   TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    peer_session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    granted_by   UUID REFERENCES users(id),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ,
    CHECK (session_id <> peer_session_id)
);

CREATE UNIQUE INDEX IF NOT EXISTS session_peer_shares_pair
    ON session_peer_shares (session_id, peer_session_id)
    WHERE revoked_at IS NULL;

CREATE INDEX IF NOT EXISTS session_peer_shares_peer
    ON session_peer_shares (peer_session_id)
    WHERE revoked_at IS NULL;
