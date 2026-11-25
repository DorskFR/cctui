//! Search types shared with other domains.

/// One event's chunks, ready to insert.
pub struct Chunked {
    pub event_id: i64,
    pub session_id: String,
    pub bodies: Vec<String>,
}
