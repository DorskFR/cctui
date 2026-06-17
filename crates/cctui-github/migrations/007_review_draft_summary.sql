-- GH-AGENT-2: the agent review tool's `review_summary` needs a place to stash a
-- draft-level summary (the overall review body) alongside the per-line comments.
-- A nullable `summary` column on `review_drafts` carries it; `review_summary`
-- set/appends it and sets the verdict. NULL means "no summary yet" — the human
-- (GH-VIEW-5) decides whether to publish it as the review body.
--
-- Runs with `search_path = github` (see `cctui_github::migrate`).
ALTER TABLE review_drafts ADD COLUMN summary TEXT;
