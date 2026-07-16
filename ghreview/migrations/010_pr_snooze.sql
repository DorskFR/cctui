-- CCT-711 #16: per-(account, pull request) snooze state.
--
-- A snoozed PR is hidden from the default pull request list (the "All" view) and
-- surfaced only in the dedicated "Snoozed" view. One row per (account, owner,
-- repo, pull_number); the row's presence means "snoozed". snoozed_at records
-- when the snooze was taken so the sync path can auto-un-snooze: any newer
-- activity (a fresh notification, review, comment, or push) whose timestamp is
-- after snoozed_at clears the snooze and returns the PR to the inbox.

CREATE TABLE IF NOT EXISTS ghreview.pr_snooze (
    account      TEXT NOT NULL,
    owner        TEXT NOT NULL,
    repo         TEXT NOT NULL,
    pull_number  INTEGER NOT NULL,
    snoozed_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (account, owner, repo, pull_number)
);

-- The default list excludes snoozed PRs by account; the snoozed view scans by
-- account too.
CREATE INDEX IF NOT EXISTS idx_pr_snooze_account
    ON ghreview.pr_snooze (account);
