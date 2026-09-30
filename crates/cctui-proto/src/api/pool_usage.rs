//! Pool quota aggregation, as rendered by the pool zone and stats panel.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// A member's share of one aggregated window.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PoolUsageWindowMember {
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub account_id: Uuid,
    pub utilization: f64,
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub resets_at: Option<DateTime<Utc>>,
    pub expected_pct: Option<f64>,
    pub ratio: Option<f64>,
}

/// When the pool runs out under measured rates.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PoolProjection {
    /// Every member at 100% at once; `None` when that never happens inside the
    /// horizon (a reset always comes first).
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub wall_at: Option<DateTime<Utc>>,
    /// The first member to hit 100%, which matters when failover is off.
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub first_member_wall_at: Option<DateTime<Utc>>,
    /// Sum of the members' measured rates, in percent points per hour.
    pub demand_pct_per_hour: f64,
    /// Shortest slope base among the members, in hours.
    pub slope_hours: f64,
    /// Lowest weighted mean headroom reached inside the horizon.
    pub min_margin_pct: f64,
}

/// One window of a pool family, aggregated.
#[derive(Debug, Clone, serde::Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PoolUsageWindow {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub model_display_name: Option<String>,
    /// Weighted mean utilization.
    pub level_pct: f64,
    /// Weighted mean of what an even spend would show now.
    pub expected_pct: f64,
    /// `level / expected`, the pool's burn against its linear budget; `None`
    /// on a window too young to rate.
    pub ratio: Option<f64>,
    /// Nearest reset among the members.
    #[cfg_attr(feature = "ts", ts(type = "string | null"))]
    pub next_reset_at: Option<DateTime<Utc>>,
    pub members: Vec<PoolUsageWindowMember>,
    pub projection: Option<PoolProjection>,
    /// Set exactly when `projection` is `None`.
    pub projection_unavailable: Option<String>,
}

