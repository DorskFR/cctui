//! Session types and pure classification shared with other domains.
use cctui_proto::adapter::SessionChild;
use cctui_proto::classifier::{Bucket, ClassifyInput, PrStatus, classify};

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
