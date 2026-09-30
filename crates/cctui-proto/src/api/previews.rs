//! `/api/v1/previews` — dev-server preview handles and tickets.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub id: String,
    pub port: u16,
    pub url: String,
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub opened_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct PreviewTicket {
    pub ticket: String,
    /// Absolute URL that redeems the ticket and lands on the preview.
    pub auth_url: String,
    pub expires_in_secs: u32,
}
