ALTER TABLE preset_hooks
    ADD COLUMN max_usd DOUBLE PRECISION CHECK (max_usd > 0),
    ADD COLUMN budget_window_hours INTEGER NOT NULL DEFAULT 24
        CHECK (budget_window_hours BETWEEN 1 AND 720);

ALTER TABLE preset_hook_deliveries DROP CONSTRAINT preset_hook_deliveries_outcome_check;
ALTER TABLE preset_hook_deliveries ADD CONSTRAINT preset_hook_deliveries_outcome_check
    CHECK (outcome IN ('dispatched', 'duplicate', 'rejected', 'rate_limited', 'over_budget'));
