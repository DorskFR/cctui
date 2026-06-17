-- GH-CONN-3: synced PR state, CI checks, and posted reviews/threads/comments.
--
-- Runs with `search_path = github` (see `cctui_github::migrate`), so every
-- unqualified table below is created in the dedicated `github` schema. These
-- are the SPINE the webhook (GH-CONN-2) and reconcile (GH-CONN-4) paths upsert
-- into, and that the inbox/diff views (GH-UI-1, GH-VIEW-*) read from.
--
-- One-directional-FK invariant (docs/github-integration.md §7.2): FKs point
-- only *into* `github.connectors` (itself a github.* table) — never back into
-- core. `DROP SCHEMA github CASCADE` removes all of this without touching core.
--
-- Identity model: GitHub's own numeric ids are the natural keys. A pull is
-- keyed by `(connector_id, repo, number)` (the human-facing PR ref) with
-- GitHub's `node_id`/`id` carried alongside; children key on GitHub's stable
-- ids so webhook + reconcile converge on the same row (idempotent upsert).

-- A synced pull request. `connector_id` scopes it to the connector that
-- tracks the repo, so removing a connector (or the whole schema) drops its PRs.
CREATE TABLE pulls (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id    UUID        NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    -- GitHub's stable global node id for the PR (GraphQL), carried for later
    -- API calls; the upsert key is (connector_id, repo, number).
    node_id         TEXT        NOT NULL,
    -- 'owner/name' slug the PR lives in.
    repo            TEXT        NOT NULL,
    -- The human-facing PR number within the repo.
    number          BIGINT      NOT NULL,
    title           TEXT        NOT NULL,
    -- 'open' | 'closed' (GitHub's `state`).
    state           TEXT        NOT NULL,
    -- Whether a closed PR was merged.
    merged          BOOLEAN     NOT NULL DEFAULT FALSE,
    draft           BOOLEAN     NOT NULL DEFAULT FALSE,
    -- GitHub's `mergeable_state` (clean|dirty|blocked|behind|unstable|…); NULL
    -- when GitHub has not computed it yet.
    mergeable_state TEXT,
    author          TEXT        NOT NULL,
    -- The head commit SHA; CI checks key off this (per head SHA).
    head_sha        TEXT        NOT NULL,
    base_ref        TEXT        NOT NULL,
    head_ref        TEXT        NOT NULL,
    -- GitHub's own created/updated timestamps (not our sync time).
    gh_created_at   TIMESTAMPTZ NOT NULL,
    gh_updated_at   TIMESTAMPTZ NOT NULL,
    -- When we last synced this row (server clock).
    synced_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (connector_id, repo, number)
);

CREATE INDEX pulls_connector_idx ON pulls (connector_id);
CREATE INDEX pulls_state_idx     ON pulls (connector_id, state);

-- A CI check (check_run or legacy commit status) for a head SHA. Keyed by
-- GitHub's external id per (connector, repo, head_sha) so a re-run with the
-- same id updates in place. Checks are deliberately keyed to the head SHA
-- rather than to a pull row: a new push rotates head_sha and naturally
-- supersedes the old checks for the inbox's "CI red" bucket.
CREATE TABLE checks (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id    UUID        NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    repo            TEXT        NOT NULL,
    head_sha        TEXT        NOT NULL,
    -- GitHub's check_run id, or 'status:<context>' for legacy commit statuses.
    external_id     TEXT        NOT NULL,
    name            TEXT        NOT NULL,
    -- 'queued' | 'in_progress' | 'completed' (check_run status).
    status          TEXT        NOT NULL,
    -- 'success' | 'failure' | 'neutral' | 'cancelled' | … ; NULL while running.
    conclusion      TEXT,
    details_url     TEXT,
    synced_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (connector_id, repo, head_sha, external_id)
);

CREATE INDEX checks_head_idx ON checks (connector_id, repo, head_sha);

-- A submitted PR review (the posted side). Keyed by GitHub's review id.
CREATE TABLE reviews (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id    UUID        NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    repo            TEXT        NOT NULL,
    pull_number     BIGINT      NOT NULL,
    -- GitHub's review id (the upsert key, scoped to the connector).
    review_id       BIGINT      NOT NULL,
    reviewer        TEXT        NOT NULL,
    -- 'approved' | 'changes_requested' | 'commented' | 'dismissed' | 'pending'.
    state           TEXT        NOT NULL,
    body            TEXT,
    -- The commit the review was submitted against.
    commit_id       TEXT,
    submitted_at    TIMESTAMPTZ,
    synced_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (connector_id, review_id)
);

CREATE INDEX reviews_pull_idx ON reviews (connector_id, repo, pull_number);

-- A review thread (a conversation anchored on a diff line). Keyed by GitHub's
-- thread node id. Resolution state drives whether the inbox still flags the PR.
CREATE TABLE review_threads (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id    UUID        NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    repo            TEXT        NOT NULL,
    pull_number     BIGINT      NOT NULL,
    -- GitHub's review-thread node id (GraphQL); the upsert key.
    thread_node_id  TEXT        NOT NULL,
    path            TEXT        NOT NULL,
    -- 'LEFT' | 'RIGHT' diff side, when anchored.
    side            TEXT,
    line            BIGINT,
    resolved        BOOLEAN     NOT NULL DEFAULT FALSE,
    synced_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (connector_id, thread_node_id)
);

CREATE INDEX review_threads_pull_idx ON review_threads (connector_id, repo, pull_number);

-- An individual review comment. Keyed by GitHub's comment id. `thread_node_id`
-- correlates it to a review_threads row when known (no FK: a comment may sync
-- before its thread, and we never want a missing thread to reject a comment).
CREATE TABLE review_comments (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    connector_id    UUID        NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    repo            TEXT        NOT NULL,
    pull_number     BIGINT      NOT NULL,
    -- GitHub's review-comment id (the upsert key, scoped to the connector).
    comment_id      BIGINT      NOT NULL,
    thread_node_id  TEXT,
    author          TEXT        NOT NULL,
    body            TEXT        NOT NULL,
    path            TEXT,
    side            TEXT,
    line            BIGINT,
    gh_created_at   TIMESTAMPTZ NOT NULL,
    gh_updated_at   TIMESTAMPTZ NOT NULL,
    synced_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (connector_id, comment_id)
);

CREATE INDEX review_comments_pull_idx   ON review_comments (connector_id, repo, pull_number);
CREATE INDEX review_comments_thread_idx ON review_comments (connector_id, thread_node_id);
