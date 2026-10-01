//! `GET /api/v1/machines/resources` — per-machine host resource snapshots.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

/// One enrolled daemon machine and its last-known resource snapshot, for the
/// Settings › Resource monitoring list and the header gauge.
#[derive(Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MachineResourcesRow {
    pub machine_id: Uuid,
    pub name: String,
    pub display_name: Option<String>,
    /// Operator-set badge hue (0-359). `None` = hash of the name.
    pub hue: Option<i16>,
    pub liveness: crate::models::MachineLiveness,
    /// When the daemon was last heard from. The tier above is derived from this,
    /// but a reader that wants to show an age needs the stamp itself.
    pub last_seen_at: DateTime<Utc>,
    /// `None` until the machine's daemon has sent a heartbeat carrying a
    /// snapshot (older daemon, non-Linux host): the gauge shows "?" then.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<crate::resources::MachineResources>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<DateTime<Utc>>,
}
