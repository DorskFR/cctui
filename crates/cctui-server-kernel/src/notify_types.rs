//! Notification types shared with other domains.
use uuid::Uuid;

pub mod kind {
    pub const APPROVAL: &str = "approval";
    pub const USER_ACTION: &str = "user_action";
    pub const NEEDS_INPUT: &str = "needs_input";
    pub const SESSION_STUCK: &str = "session_stuck";
    pub const SYSTEM_ALERT: &str = "system_alert";
    pub const INBOX_STUCK: &str = "inbox_stuck";
    pub const DISPATCH_INFO: &str = "dispatch_info";
    pub const DISPATCH_FAILED: &str = "dispatch_failed";
    pub const TEST: &str = "test";
    pub const THROTTLED: &str = "notify_throttled";
}

#[derive(Debug, Clone)]
pub struct NewNotification {
    pub user_id: Uuid,
    pub kind: &'static str,
    pub severity: &'static str,
    pub session_id: Option<String>,
    pub machine_id: Option<Uuid>,
    pub subject_kind: Option<String>,
    pub subject_id: Option<String>,
    pub collapse_key: Option<String>,
    pub title: String,
    pub body: Option<String>,
    pub tags: Option<String>,
    pub actions: serde_json::Value,
    pub blocking: bool,
    /// The `stream_events` id of the message that raised it.
    pub anchor_seq: Option<i64>,
}

impl NewNotification {
    #[must_use]
    pub const fn new(user_id: Uuid, kind: &'static str, title: String) -> Self {
        Self {
            user_id,
            kind,
            severity: "info",
            session_id: None,
            machine_id: None,
            subject_kind: None,
            subject_id: None,
            collapse_key: None,
            title,
            body: None,
            tags: None,
            actions: serde_json::Value::Array(Vec::new()),
            blocking: false,
            anchor_seq: None,
        }
    }
}
