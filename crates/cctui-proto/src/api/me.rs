//! `GET /api/v1/me` — who the presented token resolves to.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct MeResponse {
    /// Coarse role hint for the UI: `admin` | `user` | `machine`. Derived from
    /// scopes + machine id; authority itself lives in `scopes`.
    pub role: String,
    /// Always present now — everyone is a real user. Kept `Option`
    /// for webui wire-compat; never `null` in practice.
    pub user_id: Option<Uuid>,
    /// Resolved from `users.name`.
    pub user_name: Option<String>,
    pub machine_id: Option<Uuid>,
    /// Effective scopes (`key_acls` ∩ `user_acls`) for this request.
    pub scopes: Vec<String>,
    /// Non-secret fragment of the token this request authenticated with,
    /// e.g. `cctui_u_ab1234…ef34`.
    pub token_preview: String,
}
