//! `/api/v1/bookmarks` — a cross-session collection of saved messages.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "sqlx", derive(sqlx::FromRow))]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct Bookmark {
    pub id: Uuid,
    /// Source session, or `None` once that session has been deleted.
    pub session_id: Option<String>,
    /// Position of the message within the source transcript.
    pub seq: Option<i64>,
    pub message_id: Option<String>,
    pub title: String,
    /// The snapshotted message text, as Markdown.
    pub body: String,
    pub role: String,
    /// Denormalised at save time so a dead-link bookmark still says where it
    /// came from.
    pub session_name: Option<String>,
    pub note: Option<String>,
    pub message_ts: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CreateBookmark {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub seq: Option<i64>,
    #[serde(default)]
    pub message_id: Option<String>,
    pub title: String,
    pub body: String,
    pub role: String,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Message timestamp (epoch millis on the wire, as the UI carries it).
    pub message_ts: i64,
}

/// `PATCH` payload — title and note only; the snapshot itself is immutable.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UpdateBookmark {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}
