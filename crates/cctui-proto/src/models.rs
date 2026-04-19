use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lifecycle state for a session. Three non-terminal states driven by the
/// timestamp of the most recent activity, not by a heartbeat liveness probe:
///
/// - `New`: session registered, no assistant turn has arrived yet.
/// - `Active`: most recent activity within the active window.
/// - `Inactive`: no recent activity, but the session is not archived — a
///   new message or turn revives it back to `Active`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    New,
    Active,
    Inactive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub tokens_in: u64,
    pub tokens_out: u64,
    pub cost_usd: f64,
}

impl Default for TokenUsage {
    fn default() -> Self {
        Self { tokens_in: 0, tokens_out: 0, cost_usd: 0.0 }
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
        };
        let json = serde_json::to_string(&session).unwrap();
        let parsed: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.machine_id, "test-machine");
        assert_eq!(parsed.status, SessionStatus::Active);
    }
}
