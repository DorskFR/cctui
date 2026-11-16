use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct Notification {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub severity: String,
    pub session_id: Option<String>,
    pub machine_id: Option<Uuid>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<String>,
    pub collapse_key: Option<String>,
    pub title: String,
    pub body: Option<String>,
    pub tags: Option<String>,
    /// `[{id, label, style?, requires_login?}]`.
    pub actions: serde_json::Value,
    /// `pending` | `delivered` | `read` | `acted` | `resolved` | `expired`.
    pub state: String,
    pub repeat_count: i32,
    pub decision: Option<String>,
    pub decision_note: Option<String>,
    pub acted_channel: Option<String>,
    pub acted_by: Option<Uuid>,
    pub resolved_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub read_at: Option<DateTime<Utc>>,
    pub acted_at: Option<DateTime<Utc>>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct NotificationList {
    pub notifications: Vec<Notification>,
    pub unread: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct NotificationTransition {
    pub notification: Notification,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct NotificationActRequest {
    pub action: String,
    #[serde(default)]
    pub note: Option<String>,
    /// `web` (default) or `tui`.
    #[serde(default)]
    pub channel: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct NotificationReadAll {
    pub read: i64,
    pub unread: i64,
}
