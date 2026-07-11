-- CCT-602: server-managed notification state.
-- Read/done/archived flags live here, layered on top of the GitHub-shaped
-- notification documents (documents WHERE kind = 'notification', key = thread id).
-- One row per (account, thread_id); absent rows mean "no local state yet".

CREATE TABLE IF NOT EXISTS ghreview.notification_state (
    account       TEXT NOT NULL,
    thread_id     TEXT NOT NULL,
    read          BOOLEAN NOT NULL DEFAULT false,
    done          BOOLEAN NOT NULL DEFAULT false,
    archived      BOOLEAN NOT NULL DEFAULT false,
    read_at       TIMESTAMPTZ,
    done_at       TIMESTAMPTZ,
    archived_at   TIMESTAMPTZ,
    -- mark-as-read is pushed back to GitHub; push_pending flags a read that has
    -- not yet been confirmed pushed. last_error records the last push failure.
    -- The poller drains push_pending rows on each tick so state is never lost.
    push_pending  BOOLEAN NOT NULL DEFAULT false,
    last_error    TEXT,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account, thread_id)
);

-- Inbox queries filter/paginate over the join of documents + state.
CREATE INDEX IF NOT EXISTS idx_notification_state_done
    ON ghreview.notification_state (account, done);
CREATE INDEX IF NOT EXISTS idx_notification_state_archived
    ON ghreview.notification_state (account, archived);
-- The poller scans for reads still owed to GitHub.
CREATE INDEX IF NOT EXISTS idx_notification_state_push_pending
    ON ghreview.notification_state (account) WHERE push_pending;
