//! Upstream provider incident readings, as served on the usage rows.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Family labels the poller knows, with their summary endpoint and human page.
///
/// `status.anthropic.com` 301s to `status.claude.com`; the client follows redirects, so either
/// spelling works — the canonical one is used directly.
pub const SOURCES: &[(&str, &str, &str)] = &[
    ("anthropic", "https://status.claude.com/api/v2/summary.json", "https://status.claude.com"),
    ("openai", "https://status.openai.com/api/v2/summary.json", "https://status.openai.com"),
];

/// Normalized severity of an upstream family. `Unknown` is a first-class state:
/// no reading yet, or a reading we could not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "ProviderIndicator"))]
pub enum Indicator {
    Unknown,
    None,
    Minor,
    Major,
    Critical,
}

impl Indicator {
    /// Statuspage's `status.indicator`, anything else read as `Unknown` rather
    /// than silently flattened to `None`.
    #[must_use]
    pub fn parse(raw: &str) -> Self {
        match raw {
            "none" => Self::None,
            "minor" => Self::Minor,
            "major" => Self::Major,
            "critical" => Self::Critical,
            _ => Self::Unknown,
        }
    }

    /// Whether this reading is worth surfacing at all: nothing is shown for a
    /// healthy family, and `Unknown` is not news either.
    #[must_use]
    pub const fn is_degraded(self) -> bool {
        matches!(self, Self::Minor | Self::Major | Self::Critical)
    }
}

/// One component the provider reports as anything but operational.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DegradedComponent {
    pub name: String,
    /// Statuspage component status (`degraded_performance`, `major_outage`, …).
    pub status: String,
}

/// One unresolved incident, as much of it as a badge can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct IncidentSummary {
    pub name: String,
    /// `none` | `minor` | `major` | `critical`.
    pub impact: String,
    /// Short public link to the incident, when the page gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub url: Option<String>,
}

/// What a client gets for one family. Serializable and cheap to clone: it is
/// embedded in every usage row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderStatus {
    /// `anthropic` | `openai`.
    pub family: String,
    pub indicator: Indicator,
    /// The provider's own one-liner ("All Systems Operational"), empty when the
    /// page gave none.
    pub description: String,
    /// The human status page, so a badge can link somewhere useful.
    pub url: String,
    pub components: Vec<DegradedComponent>,
    pub incidents: Vec<IncidentSummary>,
    /// When the poller last got an answer (a 304 counts). `None` ⇒ never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub checked_at: Option<DateTime<Utc>>,
}

impl ProviderStatus {
    /// The "we have no idea" reading for a family.
    #[must_use]
    pub fn unknown(family: &str) -> Self {
        Self {
            family: family.to_owned(),
            indicator: Indicator::Unknown,
            description: String::new(),
            url: page_url(family).to_owned(),
            components: Vec::new(),
            incidents: Vec::new(),
            checked_at: None,
        }
    }
}

/// The human status page for a family, or the empty string for one we do not
/// poll.
#[must_use]
pub fn page_url(family: &str) -> &'static str {
    SOURCES.iter().find(|(f, ..)| *f == family).map_or("", |(.., page)| *page)
}

/// Which polled family an account's `provider` value belongs to.
///
/// Deliberately stricter than `Family::from_provider`: an
/// `anthropic-compatible` credential points at somebody else's endpoint, and
/// attributing Anthropic's incidents to it would be a lie on the account row.
/// Only the first-party providers map.
#[must_use]
pub fn family_of_provider(provider: &str) -> Option<&'static str> {
    match provider {
        "anthropic" => Some("anthropic"),
        "openai" => Some("openai"),
        _ => None,
    }
}
