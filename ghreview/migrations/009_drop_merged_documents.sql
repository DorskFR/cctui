-- CCT-694: merged/closed pull requests must drop out of the viewer and stop
-- saturating storage. syncPull previously stored closed PRs verbatim and kept
-- the document forever; sync now deletes them, so purge the historical
-- merged/closed pull_request documents that accumulated before that change.
-- A merged PR has state = 'closed' with merged_at set; a plain closed PR has
-- state = 'closed'. Open PRs are untouched.

DELETE FROM ghreview.documents
WHERE kind = 'pull_request'
  AND (payload->>'state' = 'closed' OR payload->>'merged_at' IS NOT NULL);
