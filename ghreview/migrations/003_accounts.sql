-- CCT-603: multi-account support + ownership.
-- A cctui user (external identity: the cctui auth_keys user_id, or a static-mode
-- id) owns N GitHub accounts. Each account carries its own sealed PAT, rate
-- budget and poll schedule. `login` is globally unique so the documents /
-- notification_state / sync_state tables (already keyed by account = login) map
-- 1:1 to exactly one owner; ownership is then enforced by joining on login.

CREATE TABLE IF NOT EXISTS ghreview.gh_accounts (
    id                BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id           TEXT NOT NULL,
    login             TEXT NOT NULL UNIQUE,
    -- AES-256-GCM ciphertext of the PAT (v1:<nonce>:<tag>:<ct>, base64). Never
    -- returned by any API; decrypted only in the poller.
    encrypted_pat     TEXT NOT NULL,
    -- Per-account polling knobs; NULL falls back to the service defaults.
    poll_interval_ms  INTEGER,
    budget_ceiling    DOUBLE PRECISION,
    rate_limit        INTEGER,
    active            BOOLEAN NOT NULL DEFAULT true,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_gh_accounts_user
    ON ghreview.gh_accounts (user_id);

-- Ownership FK on subscriptions: a subscription belongs to a gh_account, which
-- belongs to a user. account_id is nullable for rows created before this
-- migration (single-account era); the poller/backfill ties them by login.
ALTER TABLE ghreview.subscriptions
    ADD COLUMN IF NOT EXISTS account_id BIGINT REFERENCES ghreview.gh_accounts (id) ON DELETE CASCADE;

CREATE INDEX IF NOT EXISTS idx_subscriptions_account_id
    ON ghreview.subscriptions (account_id);
