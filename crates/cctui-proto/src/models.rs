use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Lifecycle state for a session. Three non-terminal states driven by the
/// timestamp of the most recent activity, not by a heartbeat liveness probe:
///
/// - `New`: session registered, no assistant turn has arrived yet.
/// - `Active`: most recent activity within the active window.
/// - `Inactive`: no recent activity, but the session is not archived — a
///   new message or turn revives it back to `Active`.
/// - `Archived`: explicitly dismissed (manually or by the TTL reaper).
///   Hidden from the default list. A genuinely revived session (new
///   activity) returns to `Active`; an archived dead session stays hidden.
/// - `Draft`: staged-but-not-dispatched session (CCT-394). Carries its spawn
///   payload in `metadata.draft` but has no `command_id`, no daemon dispatch,
///   and no heartbeat — excluded from liveness/reaping. An explicit Launch
///   mints env fresh, dispatches a normal spawn, and removes the draft.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    New,
    Active,
    Inactive,
    Archived,
    Draft,
}

/// Coarse liveness tier for the sessions-list status dot.
///
/// Derived purely from the age of the last heartbeat; orthogonal to
/// [`SessionStatus`] — it answers "is this still warm?" not "what lifecycle
/// state".
///
/// - `Active`: heartbeat within the active window (green dot).
/// - `Stale`: alive but quiet — past the active window (orange dot).
/// - `Dead`: long inactive (no dot).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Liveness {
    Active,
    Stale,
    Dead,
}

/// Coarse liveness tier for a machine (its daemon's WS), derived from the age
/// of `machines.last_seen_at`, which the server now advances on every daemon
/// `Heartbeat` frame (CCT-255). Mirrors the session [`Liveness`] tiers but
/// names them in machine terms:
///
/// - `Online`: a heartbeat arrived within the active window.
/// - `Stale`: quiet but not yet declared dead — past the active window.
/// - `Offline`: no heartbeat for the full dead window (daemon gone).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum MachineLiveness {
    Online,
    Stale,
    /// Default tier — a freshly-fetched row before its `last_seen_at` is mapped
    /// is treated as offline until proven otherwise.
    #[default]
    Offline,
}

/// Why a session wants the user's eyes, surfaced as a glyph.
///
/// Derived from the classifier; today only the "blocked" bucket is
/// surfaced (the ✋ "needs input" hand).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum Attention {
    NeedsInput,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Session {
    pub id: String,
    pub parent_id: Option<String>,
    pub account_id: Option<String>,
    pub machine_id: String,
    pub working_dir: String,
    pub status: SessionStatus,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
    pub metadata: serde_json::Value,
    /// Adapter that produced this session (e.g. `"claude-code"`, `"codex"`).
    /// Optional in the wire shape for back-compat with rows persisted before
    /// the `adapter_id` column was added; v0 server fills with `"claude-code"`
    /// for legacy rows so this is always populated in fresh data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<crate::adapter::AdapterId>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TokenUsage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
    #[serde(default)]
    pub cache_read_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
}

impl Default for TokenUsage {
    fn default() -> Self {
        Self {
            tokens_in: 0,
            tokens_out: 0,
            cost_usd: 0.0,
            cache_read_tokens: 0,
            cache_creation_tokens: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_status_serializes_to_snake_case() {
        let json = serde_json::to_string(&SessionStatus::Active).unwrap();
        assert_eq!(json, r#""active""#);
        let json = serde_json::to_string(&SessionStatus::Inactive).unwrap();
        assert_eq!(json, r#""inactive""#);
        let json = serde_json::to_string(&SessionStatus::New).unwrap();
        assert_eq!(json, r#""new""#);
        let json = serde_json::to_string(&SessionStatus::Draft).unwrap();
        assert_eq!(json, r#""draft""#);
    }

    #[test]
    fn session_roundtrips_json() {
        let session = Session {
            id: "test-session-id".into(),
            parent_id: None,
            account_id: None,
            machine_id: "test-machine".into(),
            working_dir: "/tmp".into(),
            status: SessionStatus::Active,
            registered_at: Utc::now(),
            last_heartbeat: Utc::now(),
            metadata: serde_json::json!({"git_branch": "main"}),
            adapter_id: Some(crate::adapter::AdapterId::new("claude-code")),
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.machine_id, "test-machine");
        assert_eq!(parsed.status, SessionStatus::Active);
    }
}
