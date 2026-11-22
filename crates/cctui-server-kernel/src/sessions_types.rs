//! Session types and pure classification shared with other domains.
use axum::http::StatusCode;
use cctui_proto::adapter::SessionChild;
use cctui_proto::classifier::{Bucket, ClassifyInput, PrStatus, classify};
use cctui_proto::thread::ThreadCursor;
use chrono::{DateTime, Utc};

use crate::auth::AuthContext;
use crate::error::AppError;

/// Classify a live session into its bucket from the persisted signals.
///
/// `children` are the session's linked-PR references (`sessions.children`): an
/// idle session is blocked while one of them is an OPEN PR awaiting attention
/// per the `pr_cache` the GitHub connector maintains.
#[must_use]
#[allow(clippy::implicit_hasher)]
pub fn bucket_from_signals(
    tempo: Option<&str>,
    agent_state: Option<&str>,
    activity: Option<&str>,
    soft_limit_blocked: Option<&str>,
    children: &[SessionChild],
    pr_cache: &std::collections::HashMap<String, PrStatus<'_>>,
) -> Bucket {
    let input = ClassifyInput {
        tempo,
        state: agent_state,
        activity,
        children,
        q: None,
        soft_limit_blocked,
        ended: false,
    };
    classify(&input, pr_cache)
}

/// What `archive_one` did with a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveOutcome {
    Archived,
    /// The session is pinned and `force` was not set: nothing was touched.
    SkippedPinned,
}

#[derive(Debug, Default, serde::Deserialize)]
pub struct ConversationQuery {
    /// Newest `limit` events only; absent = full transcript.
    pub limit: Option<i64>,
    /// Exclusive upper `seq` bound for paging back.
    pub before: Option<i64>,
    /// Exclusive lower `seq` bound for delta catch-up.
    pub after: Option<i64>,
    /// Which end of the range `limit` takes: `desc` (default) keeps the newest
    /// events, `asc` the oldest. The response is always oldest-first.
    #[serde(default)]
    pub order: ConversationOrder,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationOrder {
    #[default]
    Desc,
    Asc,
}

/// `(id, client payload, created_at, turn_id)`: a stored row the client can
/// render, in the query's own order (newest-first for `Desc`).
pub type RenderableRow = (i64, serde_json::Value, DateTime<Utc>, Option<uuid::Uuid>);

/// A transcript's identity line.
#[derive(Debug, Clone, Default)]
pub struct Header {
    pub session_id: String,
    pub name: Option<String>,
    pub adapter: Option<String>,
    pub machine: Option<String>,
    pub state: &'static str,
}

/// A rendered page, with the cursors a caller needs to ask for the next one.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Rendered {
    pub text: String,
    pub events: usize,
    /// `seq` of the oldest event in the page; pass it as `before` to page back.
    pub first_seq: Option<i64>,
    /// `seq` of the newest event in the page; pass it as `after` for a delta.
    pub last_seq: Option<i64>,
    /// The byte budget cut the page short, so older events are missing.
    pub truncated: bool,
}

/// A rendered page of a follow-up line.
#[derive(Debug, Clone)]
pub struct RenderedThread {
    pub text: String,
    pub events: usize,
    /// Where to resume for older entries: the page's own cursor, or the oldest
    /// entry kept when the budget cut the page short.
    pub next_cursor: Option<ThreadCursor>,
    pub truncated: bool,
}

/// The requested session itself is never checked: the route already did.
#[derive(Clone, Copy)]
pub enum Reader<'a> {
    Caller(&'a AuthContext),
    Owner(Option<uuid::Uuid>),
}

/// A `cursor` query value; blank is no cursor.
pub fn parse_cursor(raw: Option<&str>) -> Result<Option<ThreadCursor>, AppError> {
    raw.map(str::trim)
        .filter(|c| !c.is_empty())
        .map(|c| {
            ThreadCursor::parse(c).ok_or_else(|| {
                AppError::new(StatusCode::BAD_REQUEST, "cursor must be <generation>:<seq>")
            })
        })
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Followed {
    pub followup_id: uuid::Uuid,
    pub command_id: uuid::Uuid,
    pub to: Option<String>,
}
