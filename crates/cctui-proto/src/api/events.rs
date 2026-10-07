//! `/api/v1/events` — the lifecycle event log, newest first.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

/// One recorded lifecycle transition. `summary` is rendered at insert time and
/// `detail` keeps the denormalised `session_name` / `machine_label`, so a row
/// stays readable once its subject is gone and the id columns are null.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct EventRecord {
    pub id: i64,
    pub occurred_at: DateTime<Utc>,
    /// `<subject>.<verb>`, e.g. `session.ended`; clients render unknown kinds generically.
    pub kind: String,
    /// `info` | `warn` | `error`.
    pub severity: String,
    pub session_id: Option<String>,
    pub machine_id: Option<Uuid>,
    pub user_id: Option<Uuid>,
    /// `user:<id>` | `daemon` | `reaper` | `system` | `agent:<session>`.
    pub actor: String,
    pub summary: String,
    pub detail: serde_json::Value,
}

/// A keyset page. Pass the last row's `id` back as `before` for the next one.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct EventPage {
    pub events: Vec<EventRecord>,
    pub has_more: bool,
}
