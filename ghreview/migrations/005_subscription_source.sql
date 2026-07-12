-- CCT-657: record how a subscription came to exist so auto-created rows can be
-- distinguished from explicit user subscriptions (e.g. auto-unsubscribe can
-- treat a notification-sourced PR differently from one a user asked for).
--
-- Values are open-ended text: 'user' (explicit API subscribe), 'repo' (backfilled
-- from a repo's open PRs, CCT-656), 'notification' (auto-subscribed from a
-- participating notification, CCT-657). NULL for rows created before this
-- migration or by paths that do not record an origin.

ALTER TABLE ghreview.subscriptions
    ADD COLUMN IF NOT EXISTS source TEXT;
