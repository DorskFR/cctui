use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(test, derive(TS), ts(export))]
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
    /// The `stream_events` id of the message that raised it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_seq: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct NotificationList {
    pub notifications: Vec<Notification>,
    pub unread: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct NotificationTransition {
    pub notification: Notification,
    pub applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
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
#[cfg_attr(test, derive(TS), ts(export))]
pub struct NotificationReadAll {
    pub read: i64,
    pub unread: i64,
}

/// Per-user settings under `user_settings.data.notifications`. Stored sparse: a channel or
/// a `routes` cell left out takes the server default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(default)]
pub struct NotificationSettings {
    pub enabled: bool,
    pub sound: bool,
    /// Keyed by channel: `in_app`, `web_push`, `ntfy`, `email`.
    pub channels: BTreeMap<String, NotificationChannelPref>,
    /// Kind, then channel.
    pub routes: BTreeMap<String, BTreeMap<String, NotificationStage>>,
    pub quiet_hours: Option<QuietHours>,
    pub max_push_per_hour: u32,
}

pub const DEFAULT_MAX_PUSH_PER_HOUR: u32 = 20;

impl Default for NotificationSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            sound: true,
            channels: BTreeMap::new(),
            routes: BTreeMap::new(),
            quiet_hours: None,
            max_push_per_hour: DEFAULT_MAX_PUSH_PER_HOUR,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct NotificationChannelPref {
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum NotificationStage {
    Off,
    Now,
    After { minutes: u32 },
}

/// `start` and `end` are `HH:MM` local to `tz` (IANA); a window may cross midnight.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct QuietHours {
    pub start: String,
    pub end: String,
    pub tz: String,
}

/// Secrets in `config` are never returned: an ntfy token shows as `has_token`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct NotificationEndpoint {
    pub id: Uuid,
    pub channel: String,
    pub label: Option<String>,
    pub config: serde_json::Value,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub last_ok_at: Option<DateTime<Utc>>,
}

/// `config` per channel: ntfy `{url, topic, token?}`, `web_push` the browser's
/// `PushSubscription` (or `{subscription}`), email `{address}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct CreateNotificationEndpoint {
    pub channel: String,
    #[serde(default)]
    pub label: Option<String>,
    pub config: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct NotificationTestSent {
    pub delivery_id: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct VapidKey {
    /// Uncompressed P-256 point, base64url: `pushManager.subscribe`'s `applicationServerKey`.
    pub public_key: String,
}
