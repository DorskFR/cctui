-- CCT-687: notifications now sync with all=true (the full GitHub inbox: read +
-- unread, not just unread) instead of all=false. That changes the /notifications
-- query identity, so the ETag/Last-Modified stored from the old all=false walk is
-- stale. Clear it so the next poll does a full re-walk (fetching all ~380 inbox
-- threads) instead of short-circuiting on a 304 against the old conditional cache.

UPDATE ghreview.sync_state
SET etag = NULL, last_modified = NULL
WHERE kind = 'notification';
