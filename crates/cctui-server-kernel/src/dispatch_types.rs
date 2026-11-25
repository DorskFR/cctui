//! Execution target types shared with other domains.
use cctui_proto::models::MachineLiveness;
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;

use axum::Json;
use axum::http::StatusCode;
use cctui_proto::adapter::{PermissionMode, SessionSpec};
use cctui_proto::api::soft_limit::{Decision, SoftLimits, UsageWindow};
use cctui_proto::api::{ApiError, DispatchResponse};
use uuid::Uuid;

use crate::bus::BusError;
use crate::commands::FailedSpawnRow;
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

/// The authorized target and prepared worker input, without workspace storage.
pub struct MachineSpawn {
    pub machine_id: Uuid,
    pub command_id: Uuid,
    pub token_key: Uuid,
    pub spec: SessionSpec,
    pub failed: FailedSpawnRow,
    /// Deliver on the connection that announced this session, not the
    /// machine's newest one: dispatched workers share one machine identity.
    pub route_session: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("could not persist machine launch: {0}")]
    Storage(#[from] sqlx::Error),
    #[error(transparent)]
    Delivery(#[from] BusError),
    #[error("the daemon refused the spawn: {0}")]
    Refused(String),
    #[error("hook policy: {0}")]
    Policy(String),
}

/// Pre-mint the session identity only for a harness that accepts it.
///
/// Other harnesses echo the command key when announcing their own session ID,
/// allowing the host to rebind temporary credentials and launch intents.
#[must_use]
pub fn spawn_ids(adapter_id: &str, command_id: Uuid) -> (Option<Uuid>, Uuid) {
    let pre_session_id = (adapter_id == "claude-code").then(Uuid::new_v4);
    (pre_session_id, pre_session_id.unwrap_or(command_id))
}

/// What a launch tells the user when the daemon's machine is offline.
pub const DAEMON_OFFLINE: &str = "daemon for that machine is offline — start `cctui-daemon` first";

/// Execution identity to bind before a machine launch.
pub struct AccountTarget {
    pub uid: Uuid,
    pub adapter_id: String,
    pub token_session_id: String,
}

/// The account binding outcome and the session settings it may have rewritten.
pub struct BoundAccount {
    /// Request env plus any minted gateway env.
    pub env: std::collections::BTreeMap<String, String>,
    /// The session's model after per-account alias remapping (e.g. `opus` →
    /// `claude-opus-4-8[1m]`).
    pub model: Option<String>,
    pub effort: Option<String>,
    pub permission_mode: Option<cctui_proto::adapter::PermissionMode>,
    pub account_choice: Option<String>,
}

/// The parent session's launch context, everything a child inherits.
#[derive(Debug)]
pub struct Parent {
    pub session_id: String,
    pub machine_uuid: Uuid,
    pub working_dir: Option<String>,
    pub user_id: Uuid,
    pub permission_mode: Option<PermissionMode>,
}

#[derive(serde::Deserialize)]
pub struct ChildCompletion {
    pub text: String,
}

/// Slots and remaining depth visible to the session usage tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ChildSlots {
    pub used: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth_left: Option<u32>,
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct LimitsQuery {
    /// Ask about one model instead of the session's current one.
    pub model: Option<String>,
}

/// The pinned account, named but never attributed: no owner, no credential.
#[derive(Debug, serde::Serialize)]
pub struct AccountView {
    pub id: Uuid,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    pub provider: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct WindowView {
    #[serde(flatten)]
    pub window: UsageWindow,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pace: Option<cctui_proto::api::pace::Pace>,
}

#[derive(Debug, PartialEq, Eq, serde::Serialize)]
pub struct DecisionView {
    pub allow: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

impl From<Decision> for DecisionView {
    fn from(d: Decision) -> Self {
        match d {
            Decision::Allow => {
                Self { allow: true, retry_after_secs: None, reason: None, key: None }
            }
            Decision::Block { retry_after_secs, reason, key } => Self {
                allow: false,
                retry_after_secs: Some(retry_after_secs),
                reason: Some(reason),
                key: Some(key),
            },
        }
    }
}

/// A block already written onto the session row by an earlier refusal.
#[derive(Debug, serde::Serialize)]
pub struct BlockView {
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// Where a `CctuiAgent` child of one adapter would run: the account it would
/// land on, or why its spawn would be refused.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChildLandingView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub via_pool: bool,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub unbound: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct SessionLimits {
    pub session_id: String,
    pub account: AccountView,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub windows: Vec<WindowView>,
    pub caps: SoftLimits,
    pub spend: BTreeMap<String, f64>,
    pub decision: DecisionView,
    pub per_model: BTreeMap<String, DecisionView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub block: Option<BlockView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<ChildSlots>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub child_accounts: BTreeMap<String, ChildLandingView>,
    pub age_secs: u64,
    pub stale: bool,
}

#[derive(Debug, sqlx::FromRow)]
pub struct Launch {
    pub command_id: Uuid,
    pub session_id: String,
    pub registered_session_id: Option<String>,
    pub machine_id: Uuid,
    pub user_id: Uuid,
    pub acting_user_id: Uuid,
    pub adapter_id: String,
    pub working_dir: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub state: String,
    pub detail: Option<String>,
}

impl Launch {
    #[must_use]
    pub fn failed_row(&self) -> FailedSpawnRow {
        FailedSpawnRow {
            session_id: self.session_id.clone(),
            machine_id: self.machine_id,
            user_id: self.user_id,
            adapter_id: self.adapter_id.clone(),
            working_dir: self.working_dir.clone(),
            name: self.name.clone(),
            model: self.model.clone(),
            effort: self.effort.clone(),
        }
    }
    #[must_use]
    pub fn failed(&self) -> bool {
        matches!(self.state.as_str(), "refused" | "delivery_failed")
    }
}

pub type DispatchResult<T> = Result<T, (StatusCode, Json<ApiError>)>;

pub type Forwarded = (StatusCode, Json<DispatchResponse>);
