use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;
use uuid::Uuid;

pub use crate::thread::FollowupReason;

pub const IDEMPOTENCY_KEY_MAX: usize = 128;

const fn archive_by_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct FollowupRequest {
    #[serde(default)]
    pub instruction: String,
    pub reason: FollowupReason,
    #[serde(default = "archive_by_default")]
    pub archive_source: bool,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FollowupState {
    Pending,
    Registered,
    Failed,
}

impl FollowupState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Registered => "registered",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "pending" => Some(Self::Pending),
            "registered" => Some(Self::Registered),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct FollowupResponse {
    pub followup_id: Uuid,
    pub from: String,
    pub to: Option<String>,
    pub state: FollowupState,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_defaults_to_no_instruction_and_archiving_the_source() {
        let req: FollowupRequest =
            serde_json::from_str(r#"{"reason":"cold","idempotency_key":"k1"}"#).unwrap();
        assert_eq!(
            req,
            FollowupRequest {
                instruction: String::new(),
                reason: FollowupReason::Cold,
                archive_source: true,
                idempotency_key: "k1".into(),
            }
        );
        let back: FollowupRequest =
            serde_json::from_str(&serde_json::to_string(&req).unwrap()).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn a_request_keeps_an_explicit_instruction_and_no_archive() {
        let req: FollowupRequest = serde_json::from_value(serde_json::json!({
            "instruction": "fix the test",
            "reason": "manual",
            "archive_source": false,
            "idempotency_key": "k2",
        }))
        .unwrap();
        assert_eq!(req.instruction, "fix the test");
        assert_eq!(req.reason, FollowupReason::Manual);
        assert!(!req.archive_source);
    }

    #[test]
    fn a_response_carries_nulls_for_an_unknown_target() {
        let res = FollowupResponse {
            followup_id: Uuid::nil(),
            from: "a".into(),
            to: None,
            state: FollowupState::Pending,
            error: None,
        };
        let json = serde_json::to_value(&res).unwrap();
        assert_eq!(json["state"], "pending");
        assert!(json["to"].is_null() && json["error"].is_null());
        assert_eq!(serde_json::from_value::<FollowupResponse>(json).unwrap(), res);
    }

    #[test]
    fn states_parse_back_from_their_names() {
        for s in [FollowupState::Pending, FollowupState::Registered, FollowupState::Failed] {
            assert_eq!(FollowupState::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), s.as_str());
        }
        assert_eq!(FollowupState::parse("done"), None);
    }
}
