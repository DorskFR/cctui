-- Previews must resolve on any replica: the daemon's WS lands on one pod but
-- browser requests for cctui-pv-<id> can hit any of them.
CREATE TABLE IF NOT EXISTS previews (
    id text PRIMARY KEY,
    session_id text NOT NULL,
    user_id uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    machine_id uuid NOT NULL,
    port integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    -- Set when the owning daemon disconnects; a reconnect within the grace
    -- period clears it, so a rolling restart does not drop open previews.
    detached_at timestamptz
);

CREATE INDEX IF NOT EXISTS previews_session_idx ON previews (session_id);
CREATE INDEX IF NOT EXISTS previews_machine_idx ON previews (machine_id);
CREATE UNIQUE INDEX IF NOT EXISTS previews_session_port_idx ON previews (session_id, port);

-- Ticket nonces are single-use cluster-wide: the POST that mints and the
-- redemption that burns can land on different pods.
CREATE TABLE IF NOT EXISTS preview_tickets_used (
    nonce text PRIMARY KEY,
    expires_at timestamptz NOT NULL
);

CREATE INDEX IF NOT EXISTS preview_tickets_used_expires_idx ON preview_tickets_used (expires_at);
