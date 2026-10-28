//! Execution target liveness independent of workspace presentation.
use cctui_proto::models::MachineLiveness;
use chrono::{DateTime, Utc};
pub const ONLINE_SECS: i64 = 5 * 60;
pub const OFFLINE_SECS: i64 = 60 * 60;
/// Map a persisted heartbeat onto the existing machine liveness tiers.
#[must_use]
pub fn derive_liveness(last_seen_at: DateTime<Utc>) -> MachineLiveness {
    let age = (Utc::now() - last_seen_at).num_seconds();
    if age < ONLINE_SECS {
        MachineLiveness::Online
    } else if age < OFFLINE_SECS {
        MachineLiveness::Stale
    } else {
        MachineLiveness::Offline
    }
}

pub mod enrollment;
