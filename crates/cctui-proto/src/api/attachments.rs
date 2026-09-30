//! `/api/v1/sessions/{id}/files` — mid-chat attachments.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionAttachment {
    pub id: uuid::Uuid,
    pub session_id: String,
    pub message_id: Option<String>,
    pub name: String,
    pub hash: String,
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub size: i64,
    pub content_type: Option<String>,
    #[cfg_attr(feature = "sqlx", sqlx(rename = "created_at_ms"))]
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub created_at: i64,
    /// The session's machine, once it has registered: what the webui needs to
    /// fall back to the staged copy through `/machines/{id}/fs/file`.
    pub machine_id: Option<String>,
}

