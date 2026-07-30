-- The poller sweeps one account per tick; without this it reads the whole table.
CREATE INDEX IF NOT EXISTS idx_subscriptions_account_active
    ON ghreview.subscriptions (account, id)
    WHERE active;
