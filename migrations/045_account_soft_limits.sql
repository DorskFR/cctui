-- CCT-411: per-account soft limit on the 5h / 7d subscription windows.
--
-- A cctui account (the Anthropic OAuth subscription kind) is often shared with
-- the user's own interactive Claude Code and other workloads. These columns let
-- cctui cap its OWN dispatched share of each window so it leaves headroom for the
-- human, while bypassing the cap when a window is about to reset anyway.
--
--   * `soft_limit_5h_pct`        — max % of the 5h window cctui will consume
--                                  before it refuses to proxy more inference.
--   * `soft_limit_7d_pct`        — same for the 7d weekly window.
--   * `soft_limit_bypass_minutes`— if a window's `resets_at` is within this many
--                                  minutes, ignore that window's cap.
--
-- All nullable. NULL/unset on a window ⇒ no soft limit on it (prior behaviour).
-- This is a soft, self-imposed cap on cctui's share, distinct from Anthropic's
-- hard limit; enforced at the gateway passthrough (routes/gateway.rs).

ALTER TABLE oauth_accounts
    ADD COLUMN IF NOT EXISTS soft_limit_5h_pct         INT,
    ADD COLUMN IF NOT EXISTS soft_limit_7d_pct         INT,
    ADD COLUMN IF NOT EXISTS soft_limit_bypass_minutes INT;
