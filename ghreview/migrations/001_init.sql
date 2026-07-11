-- CCT-601: sync daemon foundation.
-- Everything lives in a dedicated schema so gh-review coexists with the cctui
-- server's tables in the same database pod. The runner sets search_path before
-- applying files; statements below are additionally IF NOT EXISTS so a partial
-- apply is safe to retry.

CREATE SCHEMA IF NOT EXISTS ghreview;

-- What to poll. One row per (account, kind, target); target is null for the
-- account-wide feeds (e.g. notifications).
CREATE TABLE IF NOT EXISTS ghreview.subscriptions (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account     TEXT NOT NULL,
    kind        TEXT NOT NULL,
    target      TEXT,
    active      BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account, kind, target)
);

-- Envelope + verbatim GitHub payload. `key` is the stable identity within a
-- (account, kind): "owner/repo" for repos, "owner/repo#number" for pulls, the
-- notification thread id for notifications.
CREATE TABLE IF NOT EXISTS ghreview.documents (
    account     TEXT NOT NULL,
    kind        TEXT NOT NULL,
    key         TEXT NOT NULL,
    etag        TEXT,
    synced_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    payload     JSONB NOT NULL,
    PRIMARY KEY (account, kind, key)
);

CREATE INDEX IF NOT EXISTS idx_documents_payload_gin
    ON ghreview.documents USING gin (payload);
CREATE INDEX IF NOT EXISTS idx_documents_kind_updated
    ON ghreview.documents (kind, updated_at DESC);

-- Per-subscription poll bookkeeping: conditional-request etag, opaque cursor
-- (notifications Last-Modified / poll interval), and the last observed rate
-- budget snapshot for the account.
CREATE TABLE IF NOT EXISTS ghreview.sync_state (
    account         TEXT NOT NULL,
    kind            TEXT NOT NULL,
    target          TEXT NOT NULL DEFAULT '',
    etag            TEXT,
    cursor          TEXT,
    last_modified   TEXT,
    poll_interval_s INTEGER,
    last_status     INTEGER,
    last_synced_at  TIMESTAMPTZ,
    rate_limit      INTEGER,
    rate_remaining  INTEGER,
    rate_reset_at   TIMESTAMPTZ,
    PRIMARY KEY (account, kind, target)
);
