-- Passkeys (WebAuthn) as a second way into the webui, beside the bearer token.
--
-- Login is *usernameless*: the browser discovers which credential to offer, so
-- the credential itself must carry the user handle. `webauthn_credentials.user_id`
-- is exactly the `users.id` we hand the authenticator as the user handle at
-- registration, and read back out of the assertion at login.
--
-- Nothing here replaces the token path: a passkey assertion mints an ordinary,
-- expiring `auth_keys` row (kind `passkey`) and that token rides the existing
-- HttpOnly cookie. The authorization model is untouched.
CREATE TABLE webauthn_credentials (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- Raw credential id as the authenticator reports it. UNIQUE server-wide:
    -- one physical credential belongs to exactly one user.
    credential_id BYTEA       NOT NULL UNIQUE,
    -- The serialized `webauthn_rs::prelude::Passkey` (public key, signature
    -- counter, backup flags). Opaque to SQL on purpose — the crate owns its
    -- shape and migrates it, we only store and hand it back.
    passkey       JSONB       NOT NULL,
    label         TEXT        NOT NULL,
    -- `credProps.rk` from registration: false means the authenticator did NOT
    -- store a discoverable credential, so this key cannot drive the
    -- usernameless login and the UI says so rather than silently failing.
    discoverable  BOOLEAN     NOT NULL DEFAULT true,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ
);

CREATE INDEX webauthn_credentials_by_user ON webauthn_credentials (user_id);

-- In-flight ceremonies. In the DB rather than in memory so a challenge
-- survives a restart and works across replicas; single-use (deleted on finish)
-- and swept on the next start.
CREATE TABLE webauthn_challenges (
    id         UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- NULL for the login ceremony, which by construction has no user yet.
    user_id    UUID        REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT        NOT NULL,
    state      JSONB       NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT webauthn_challenges_kind CHECK (kind IN ('register', 'authenticate'))
);

CREATE INDEX webauthn_challenges_expiry ON webauthn_challenges (expires_at);
