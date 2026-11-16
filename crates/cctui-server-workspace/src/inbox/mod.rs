pub mod chain;
pub mod claim;
#[cfg(test)]
mod claim_tests;
pub mod deliver;
pub mod guards;
#[cfg(test)]
mod guards_tests;
pub mod preset;
#[cfg(test)]
mod preset_tests;
pub mod queued;
pub mod schedule;
pub mod store;
#[cfg(test)]
pub mod store_tests;
pub mod timer;

use serde::{Deserialize, Serialize};

pub const RETENTION_DAYS: i32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    N8n,
    Webhook,
    Schedule,
    Timer,
    Outcome,
    Approval,
    Human,
}

impl Source {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::N8n => "n8n",
            Self::Webhook => "webhook",
            Self::Schedule => "schedule",
            Self::Timer => "timer",
            Self::Outcome => "outcome",
            Self::Approval => "approval",
            Self::Human => "human",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "n8n" => Self::N8n,
            "webhook" => Self::Webhook,
            "schedule" => Self::Schedule,
            "timer" => Self::Timer,
            "outcome" => Self::Outcome,
            "approval" => Self::Approval,
            "human" => Self::Human,
            _ => return None,
        })
    }

    #[must_use]
    pub const fn requires_source_id(self) -> bool {
        matches!(self, Self::N8n | Self::Webhook)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Pending,
    Claimed,
    Delivered,
    Cancelled,
    Dead,
}

impl Status {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::Delivered => "delivered",
            Self::Cancelled => "cancelled",
            Self::Dead => "dead",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "pending" => Self::Pending,
            "claimed" => Self::Claimed,
            "delivered" => Self::Delivered,
            "cancelled" => Self::Cancelled,
            "dead" => Self::Dead,
            _ => return None,
        })
    }
}

#[must_use]
pub fn session_lane_key(session_id: &str) -> String {
    format!("session:{session_id}")
}

#[must_use]
pub fn preset_lane_key(preset_id: uuid::Uuid, topic: &str) -> String {
    format!("preset:{preset_id}:{topic}")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCES: [Source; 7] = [
        Source::N8n,
        Source::Webhook,
        Source::Schedule,
        Source::Timer,
        Source::Outcome,
        Source::Approval,
        Source::Human,
    ];

    #[test]
    fn source_and_status_round_trip_their_wire_names() {
        for source in SOURCES {
            assert_eq!(Source::parse(source.as_str()), Some(source));
            assert_eq!(serde_json::to_value(source).unwrap(), source.as_str());
        }
        for status in
            [Status::Pending, Status::Claimed, Status::Delivered, Status::Cancelled, Status::Dead]
        {
            assert_eq!(Status::parse(status.as_str()), Some(status));
            assert_eq!(serde_json::to_value(status).unwrap(), status.as_str());
        }
        assert_eq!(Source::parse("github"), None);
        assert_eq!(Status::parse("sent"), None);
    }

    #[test]
    fn only_external_sources_require_a_source_id() {
        let required: Vec<Source> =
            SOURCES.into_iter().filter(|s| s.requires_source_id()).collect();
        assert_eq!(required, [Source::N8n, Source::Webhook]);
    }

    #[test]
    fn session_lanes_are_keyed_by_session_id() {
        assert_eq!(session_lane_key("s-1"), "session:s-1");
    }

    #[test]
    fn preset_lanes_are_keyed_by_preset_and_topic() {
        let id = uuid::Uuid::nil();
        assert_eq!(preset_lane_key(id, "ci"), format!("preset:{id}:ci"));
    }
}
