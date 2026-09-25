-- Merged/closed pull requests must drop out of the viewer and stop
-- saturating storage. Sync deletes them; this purges the merged/closed
-- pull_request documents stored before it did.
-- A merged PR has state = 'closed' with merged_at set; a plain closed PR has
-- state = 'closed'. Open PRs are untouched.

DELETE FROM ghreview.documents
WHERE kind = 'pull_request'
  AND (payload->>'state' = 'closed' OR payload->>'merged_at' IS NOT NULL);
