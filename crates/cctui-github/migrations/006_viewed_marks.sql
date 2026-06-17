-- GH-VIEW-6: blob-keyed "reviewed" marks (docs/github-integration.md §6.2).
--
-- Runs with `search_path = github` (see `cctui_github::migrate`), so the
-- unqualified table below lands in the dedicated `github` schema.
--
-- A reviewer marks a file reviewed *relative to its blob SHA* (GitHub's per-file
-- `sha`, surfaced on `DiffFile.blob_sha` by GH-VIEW-1). The mark therefore
-- captures "I reviewed THIS content of THIS path". On a later push the diff
-- reloads with fresh blob SHAs: a file whose blob SHA still matches a stored
-- mark stays reviewed, while a file that actually changed (new blob SHA) no
-- longer matches and re-flags as unreviewed — without us re-flagging the whole
-- PR. The re-flag is therefore a pure SHA comparison the read path does, not a
-- write: we keep every mark and let the current diff decide which still apply.
--
-- One-directional-FK invariant (docs §7.2): the only FK into core is
-- `viewed_marks.user_id → public.users`; the PR ref is the same stable
-- `(connector_id, repo, pull_number)` locator the inbox/diff/drafts use (we do
-- NOT FK into github.pulls — a mark may outlive a re-sync that rotates the pull
-- row). `DROP SCHEMA github CASCADE` removes all of it, core untouched.
CREATE TABLE viewed_marks (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    -- The reviewer who marked the file (one-directional FK into core).
    user_id       UUID        NOT NULL REFERENCES public.users (id) ON DELETE CASCADE,
    -- The PR ref: connector + 'owner/name' slug + number within it.
    connector_id  UUID        NOT NULL REFERENCES connectors (id) ON DELETE CASCADE,
    repo          TEXT        NOT NULL,
    pull_number   BIGINT      NOT NULL,
    -- Head-side file path the mark applies to.
    path          TEXT        NOT NULL,
    -- The blob SHA the file had when marked reviewed. A later push that changes
    -- this file rotates its blob SHA, so the stored mark no longer matches the
    -- current diff and the file re-flags as unreviewed; unchanged files keep
    -- their matching SHA and stay reviewed.
    blob_sha      TEXT        NOT NULL,
    marked_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One mark per (user, PR, path): re-marking the same path updates its blob SHA
-- + timestamp in place rather than accumulating rows (mark idempotency). The
-- path is the natural per-file identity; the blob SHA is the *content* the mark
-- is keyed to, not part of the identity.
CREATE UNIQUE INDEX viewed_marks_one_per_file
    ON viewed_marks (user_id, connector_id, repo, pull_number, path);

-- The list read path filters by (user, PR); index it.
CREATE INDEX viewed_marks_pull_idx
    ON viewed_marks (user_id, connector_id, repo, pull_number);
