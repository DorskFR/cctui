//! `/api/v1/{resource_type}/{id}/shares` — resource share grants.

use chrono::{DateTime, Utc};
use uuid::Uuid;

/// API view of one live share grant. Safe to return — no secrets, just who the
/// resource is shared with and since when.
#[derive(Debug, serde::Serialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "ResourceShareInfo"))]
pub struct ShareInfo {
    pub resource_type: String,
    pub resource_id: Uuid,
    pub user_id: Uuid,
    /// The grantee's login (`users.name`), joined for display.
    pub user_name: String,
    pub action: String,
    pub granted_at: DateTime<Utc>,
}

/// `POST /api/v1/{resource_type}/{id}/shares` payload. `user` is the grantee,
/// accepted as either a UUID or a login (`users.name`). `action` defaults to
/// `use` (the only action today).
#[derive(Debug, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GrantShare {
    pub user: String,
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "string", optional))]
    pub action: Option<String>,
}
