-- Device-authorization login: a headless client (the TUI over SSH) starts a
-- request, shows the user a short code, and polls until a logged-in browser
-- user approves it. The device code is stored hashed — it is a bearer secret —
-- and no token is stored at all: the key is minted when the approved request is
-- claimed, exactly once.
CREATE TABLE device_auth_requests (
    id               UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    device_code_hash TEXT NOT NULL UNIQUE,
    user_code        TEXT NOT NULL UNIQUE,
    client_name      TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at       TIMESTAMPTZ NOT NULL,
    last_polled_at   TIMESTAMPTZ,
    approved_at      TIMESTAMPTZ,
    approved_by      UUID REFERENCES users(id) ON DELETE CASCADE,
    denied_at        TIMESTAMPTZ,
    claimed_at       TIMESTAMPTZ
);

CREATE INDEX device_auth_requests_expires_at_idx ON device_auth_requests (expires_at);
