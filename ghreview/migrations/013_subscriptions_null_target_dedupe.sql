-- `UNIQUE (account, kind, target)` is NULLS DISTINCT, so the account-wide feeds
-- (target IS NULL) never conflicted and every upsert inserted another row. One
-- deployment accumulated 78 identical notification subscriptions, each of them
-- polled on every tick.

WITH agg AS (
    SELECT account,
           kind,
           min(id) AS keep_id,
           bool_or(active) AS any_active,
           min(source) FILTER (WHERE source IS NOT NULL) AS best_source,
           min(account_id) FILTER (WHERE account_id IS NOT NULL) AS best_account_id
    FROM ghreview.subscriptions
    WHERE target IS NULL
    GROUP BY account, kind
),
collapsed AS (
    UPDATE ghreview.subscriptions s
    SET active = a.any_active,
        source = COALESCE(s.source, a.best_source),
        account_id = COALESCE(s.account_id, a.best_account_id)
    FROM agg a
    WHERE s.id = a.keep_id
    RETURNING s.id
)
DELETE FROM ghreview.subscriptions s
USING agg a
WHERE s.target IS NULL
  AND s.account = a.account
  AND s.kind = a.kind
  AND s.id <> a.keep_id;

UPDATE ghreview.subscriptions s
SET account_id = a.id
FROM ghreview.gh_accounts a
WHERE s.account_id IS NULL AND a.login = s.account;

ALTER TABLE ghreview.subscriptions
    DROP CONSTRAINT IF EXISTS subscriptions_account_kind_target_key;

ALTER TABLE ghreview.subscriptions
    ADD CONSTRAINT subscriptions_account_kind_target_key
    UNIQUE NULLS NOT DISTINCT (account, kind, target);
