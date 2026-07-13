-- CCT-689: the 007 re-walk still stopped after one page — GitHub caps
-- /notifications at 50 items/page regardless of per_page, and the loop treated
-- a short page as the last page. Now that pagination follows the Link header,
-- clear the conditional cache once more so the next poll ingests the whole
-- inbox instead of 304-ing against the partial walk's Last-Modified.

UPDATE ghreview.sync_state
SET etag = NULL, last_modified = NULL
WHERE kind = 'notification';
