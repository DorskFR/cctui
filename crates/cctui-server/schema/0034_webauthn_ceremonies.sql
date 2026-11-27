-- Passkey ceremonies between their start and finish, shared by every replica.
-- `key` is the SHA-256 of the ceremony id the browser holds, so the table alone
-- cannot finish a ceremony. `ceremony` holds the challenge state and never
-- leaves the server. `address` is the client network the per-address cap counts.
DROP TABLE IF EXISTS public.webauthn_challenges;

CREATE TABLE public.webauthn_ceremonies (
    key        bytea       PRIMARY KEY,
    purpose    text        NOT NULL,
    address    text,
    ceremony   jsonb       NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL
);

CREATE INDEX webauthn_ceremonies_expiry ON public.webauthn_ceremonies (expires_at);
CREATE INDEX webauthn_ceremonies_by_address ON public.webauthn_ceremonies (address, expires_at);
