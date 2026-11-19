use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

/// A signed inbound webhook bound to a launch preset. Deliveries are sent to
/// `POST /hooks/{id}` and queued on the preset's inbox lane.
///
/// `url` is where senders post, built by the server from its external URL.
/// With `max_usd` set, deliveries are refused while the sessions the hook
/// reached in the last `budget_window_hours` cost that much or more.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct Webhook {
    pub id: Uuid,
    pub user_id: Uuid,
    pub preset_id: Uuid,
    pub name: String,
    pub prompt_template: String,
    pub rate_per_minute: i32,
    pub max_usd: Option<f64>,
    pub budget_window_hours: i32,
    pub enabled: bool,
    pub created_at: DateTime<Utc>,
    pub url: String,
}

/// Body of `POST /webhooks` and `PUT /webhooks/{id}`.
///
/// `prompt_template` interpolates `{{payload.<path>}}` from the delivery's
/// JSON body, plus `{{event}}` and `{{delivery}}` from its headers.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct WebhookRequest {
    pub preset_id: Uuid,
    pub name: String,
    pub prompt_template: String,
    #[serde(default)]
    pub rate_per_minute: Option<i32>,
    #[serde(default)]
    pub max_usd: Option<f64>,
    #[serde(default)]
    pub budget_window_hours: Option<i32>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

/// Reply of `POST /webhooks` and `POST /webhooks/{id}/secret`: the only time
/// the signing secret is shown.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct WebhookSecret {
    pub webhook: Webhook,
    pub secret: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum WebhookOutcome {
    Dispatched,
    Duplicate,
    Rejected,
    RateLimited,
    OverBudget,
}

impl WebhookOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Dispatched => "dispatched",
            Self::Duplicate => "duplicate",
            Self::Rejected => "rejected",
            Self::RateLimited => "rate_limited",
            Self::OverBudget => "over_budget",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "dispatched" => Self::Dispatched,
            "duplicate" => Self::Duplicate,
            "rejected" => Self::Rejected,
            "rate_limited" => Self::RateLimited,
            "over_budget" => Self::OverBudget,
            _ => return None,
        })
    }
}

/// One entry of a webhook's delivery log. `session_id` is set once the
/// delivery's inbox event reached a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct WebhookDelivery {
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub id: i64,
    pub delivery_id: Option<String>,
    pub outcome: WebhookOutcome,
    pub detail: Option<String>,
    pub event_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub received_at: DateTime<Utc>,
}

/// Reply of `POST /hooks/{id}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct WebhookReceipt {
    pub status: WebhookOutcome,
    pub event_id: Option<Uuid>,
}

#[cfg(test)]
mod tests {
    use super::WebhookOutcome;

    #[test]
    fn outcomes_round_trip_their_wire_names() {
        for v in [
            WebhookOutcome::Dispatched,
            WebhookOutcome::Duplicate,
            WebhookOutcome::Rejected,
            WebhookOutcome::RateLimited,
            WebhookOutcome::OverBudget,
        ] {
            assert_eq!(WebhookOutcome::parse(v.as_str()), Some(v));
            assert_eq!(serde_json::to_value(v).unwrap(), v.as_str());
        }
        assert_eq!(WebhookOutcome::parse("ok"), None);
    }
}
