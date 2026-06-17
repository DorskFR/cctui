-- GH-VIEW-4: the native review-draft store (docs/github-integration.md §6.2).
--
-- Runs with `search_path = github` (see `cctui_github::migrate`), so every
-- unqualified object below lands in the dedicated `github` schema. These tables
-- hold a reviewer's *local* draft — inline comments added instantly with no
-- GitHub round-trip — until GH-VIEW-5 publishes the open draft as one batched
-- `POST /repos/{o}/{r}/pulls/{n}/reviews`.
--
-- One-directional-FK invariant (docs §7.2): the only FK into core is
-- `review_drafts.author_user_id → public.users`; everything else points within
-- `github.*`. `DROP SCHEMA github CASCADE` removes all of it, core untouched.

-- One review draft scopes a verdict + a set of inline comments to a single PR
-- for a single author. The PR ref is the same `(connector_id, repo, number)`
-- locator the inbox and diff proxy use; we do NOT FK into github.pulls because a
-- draft may outlive a re-sync that rotates the pull row, and the locator is
-- stable on its own.
CREATE TABLE review_drafts (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id    UUID        NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    -- The PR ref: 'owner/name' slug + number within it.
    repo            TEXT        NOT NULL,
    pull_number     BIGINT      NOT NULL,
    -- Author model: a human reviewer ('user') or a review agent ('agent'). For a
    -- user, `author_user_id` is the owning user (one-directional FK into core).
    -- For an agent, `author_session_id` is the cctui session that wrote it (an
    -- MCP review tool will, GH-AGENT-2). Exactly one identity column is set,
    -- matching `author_kind` (enforced by the CHECK below).
    author_kind        TEXT     NOT NULL,
    author_user_id     UUID     REFERENCES public.users (id) ON DELETE CASCADE,
    author_session_id  TEXT,
    -- The pending review verdict: 'comment' | 'approve' | 'request_changes'.
    verdict         TEXT        NOT NULL DEFAULT 'comment',
    -- 'draft' (local, editable) | 'published' (submitted to GitHub, GH-VIEW-5).
    status          TEXT        NOT NULL DEFAULT 'draft',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT review_drafts_author_kind_chk CHECK (
        (author_kind = 'user'  AND author_user_id    IS NOT NULL AND author_session_id IS NULL)
     OR (author_kind = 'agent' AND author_session_id IS NOT NULL)
    ),
    CONSTRAINT review_drafts_verdict_chk CHECK (
        verdict IN ('comment', 'approve', 'request_changes')
    ),
    CONSTRAINT review_drafts_status_chk CHECK (status IN ('draft', 'published'))
);

CREATE INDEX review_drafts_pull_idx ON review_drafts (connector_id, repo, pull_number);

-- At most one OPEN (status='draft') draft per (connector, pull, user author).
-- Published drafts are archival and don't block opening a fresh one; agent
-- drafts are not constrained here (an agent may stage alongside the human).
CREATE UNIQUE INDEX review_drafts_one_open_per_user
    ON review_drafts (connector_id, repo, pull_number, author_user_id)
    WHERE status = 'draft' AND author_kind = 'user';

-- An inline draft comment, anchored to a (path, side, line[, start_line]) the
-- reviewer selected in the diff viewer — exactly the GH-VIEW-2 DiffSelection
-- coordinates, so GH-VIEW-5 can resolve it to a GitHub anchor at publish time.
CREATE TABLE review_draft_comments (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    draft_id        UUID        NOT NULL REFERENCES review_drafts (id) ON DELETE CASCADE,
    -- Head-side file path the comment anchors on (GitHub anchors on head path).
    path            TEXT        NOT NULL,
    -- 'old' | 'new' diff side (maps to GitHub LEFT/RIGHT at publish).
    side            TEXT        NOT NULL,
    -- 1-based line on `side` (the END line for a multi-line range).
    line            BIGINT      NOT NULL,
    -- Inclusive start line for a multi-line range; NULL for a single line.
    start_line      BIGINT,
    body            TEXT        NOT NULL,
    -- Set once GH-VIEW-5 publishes: GitHub's returned review-comment id.
    github_comment_id  BIGINT,
    -- For a reply to an existing GitHub thread, the parent comment id.
    in_reply_to        BIGINT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT review_draft_comments_side_chk CHECK (side IN ('old', 'new'))
);

CREATE INDEX review_draft_comments_draft_idx ON review_draft_comments (draft_id);
