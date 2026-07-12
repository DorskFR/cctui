-- CCT-609: server-managed per-file viewed state for pull requests.
-- One row per (account, owner, repo, pull_number, path). Absent rows mean the
-- file has not been marked viewed. Layered on top of the stored pull_request
-- document (documents WHERE kind = 'pull_request', key = 'owner/repo#number').
--
-- `digest` records the file's blob sha (or a patch digest) at mark time. When a
-- new push changes the file, the sync path clears viewed to mirror GitHub, which
-- resets its native per-file viewed state on any content change.
--
-- viewed state is mirrored to github.com via the GraphQL markFileAsViewed /
-- unmarkFileAsViewed mutations. push_pending flags a change still owed to GitHub;
-- the poller drains it. github_viewed records the last state observed from
-- github.com so poll-time syncs can detect drift without clobbering local intent.

CREATE TABLE IF NOT EXISTS ghreview.viewed_state (
    account        TEXT NOT NULL,
    owner          TEXT NOT NULL,
    repo           TEXT NOT NULL,
    pull_number    INTEGER NOT NULL,
    path           TEXT NOT NULL,
    viewed         BOOLEAN NOT NULL DEFAULT false,
    -- blob sha or patch digest captured when the file was marked; NULL if the
    -- file's content was not known at mark time.
    digest         TEXT,
    -- a viewed change is pushed to github.com; push_pending flags one not yet
    -- confirmed. last_error records the last push failure.
    push_pending   BOOLEAN NOT NULL DEFAULT false,
    last_error     TEXT,
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account, owner, repo, pull_number, path)
);

-- Viewed lookups + folder/bulk ops scan one pull request at a time.
CREATE INDEX IF NOT EXISTS idx_viewed_state_pull
    ON ghreview.viewed_state (account, owner, repo, pull_number);
-- The poller scans for viewed changes still owed to github.com.
CREATE INDEX IF NOT EXISTS idx_viewed_state_push_pending
    ON ghreview.viewed_state (account) WHERE push_pending;
