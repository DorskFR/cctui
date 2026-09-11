-- Usage samples: a time series of each credential's quota windows.
--
-- Until now the only quota reading was the last upstream fetch, held in
-- process memory for a few minutes and gone at restart. That is enough to
-- draw a gauge, but not to say how fast it moves: the pace shown on a card
-- was the window average (utilization / elapsed), which on a young window
-- extrapolates a burst into a regime, and can never see a slowdown.
--
-- Every successful upstream fetch now appends one row per non-dollar window.
-- The pace math reads back a sample at least a couple of hours old (the
-- upstream percentages are integers, so a shorter base reads as 0 or as a
-- one-point jump) from the SAME window instance — matched on `resets_at` —
-- and rates on the real slope. Pool-level projections need every member's
-- slope, so they stay silent until that history exists rather than guess.
--
-- Rows outlive their usefulness after one weekly window; the writer prunes
-- anything older than 8 days per credential as it inserts.
CREATE TABLE account_usage_samples (
    provider_id UUID             NOT NULL REFERENCES account_providers(id) ON DELETE CASCADE,
    window_key  TEXT             NOT NULL,
    utilization DOUBLE PRECISION NOT NULL,
    resets_at   TIMESTAMPTZ,
    sampled_at  TIMESTAMPTZ      NOT NULL DEFAULT now(),
    PRIMARY KEY (provider_id, window_key, sampled_at)
);

-- An account's weight inside a pool aggregate: the relative size of its
-- plan. The upstream usage APIs report percentages only, never the plan
-- behind them, so a 5x and a 20x subscription both read "50%" while one holds
-- four times the headroom. The owner states the ratio; 1 means "same as the
-- others", which is right whenever the pool is homogeneous.
ALTER TABLE accounts ADD COLUMN pool_weight REAL NOT NULL DEFAULT 1.0 CHECK (pool_weight > 0);
