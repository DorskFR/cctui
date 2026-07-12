-- CCT-664: per-line inline comments + batched review publish.
--
-- A review draft accumulates per-line comments locally until the user publishes
-- them as ONE batched GitHub review (POST .../pulls/{n}/reviews). Ownership is by
-- account_id -> gh_accounts -> user_id, mirroring subscriptions. head_sha is the
-- PR head captured when the draft is opened; publish rejects if the PR head has
-- moved since (stale-head guard).

CREATE TABLE IF NOT EXISTS ghreview.review_drafts (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    account_id  BIGINT NOT NULL REFERENCES ghreview.gh_accounts (id) ON DELETE CASCADE,
    account     TEXT NOT NULL,
    owner       TEXT NOT NULL,
    repo        TEXT NOT NULL,
    pr_number   INTEGER NOT NULL,
    head_sha    TEXT,
    -- comment | approve | request_changes
    verdict     TEXT NOT NULL DEFAULT 'comment',
    body        TEXT NOT NULL DEFAULT '',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (account_id, owner, repo, pr_number)
);

CREATE INDEX IF NOT EXISTS idx_review_drafts_account
    ON ghreview.review_drafts (account_id);

CREATE TABLE IF NOT EXISTS ghreview.review_draft_comments (
    id          BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    draft_id    BIGINT NOT NULL REFERENCES ghreview.review_drafts (id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    -- LEFT (old side) | RIGHT (new side)
    side        TEXT NOT NULL DEFAULT 'RIGHT',
    line        INTEGER NOT NULL,
    -- optional multi-line range start
    start_line  INTEGER,
    start_side  TEXT,
    body        TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_review_draft_comments_draft
    ON ghreview.review_draft_comments (draft_id);
