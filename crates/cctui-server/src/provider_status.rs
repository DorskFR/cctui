//! Upstream incident awareness for the provider families we route to.
//!
//! Anthropic and OpenAI both publish a Statuspage v2 `summary.json`
//! unauthenticated. A background poller reads both at a slow cadence and keeps
//! the normalized answer in [`ProviderStatusCache`], so the request path never
//! talks to a status host: `GET /api/v1/provider-status` and the usage payloads
//! read the cache and nothing else.
//!
//! The cache has three states per family, not two. A family that has never
//! answered — or whose last fetch failed and left no earlier reading — is
//! [`Indicator::Unknown`], exactly as `account_pick` treats `usage_known`:
//! "we do not know" is never rendered as "all good", and it never blocks or
//! degrades anything. A fetch failure keeps the last good reading and only
//! stops refreshing `checked_at`, so a 500 or a timeout from the status host
//! leaves every existing behaviour unchanged.
//!
//! Politeness: each family's `ETag` is remembered and sent back as
//! `If-None-Match`, so an unchanged page costs a 304 with no body.
//!
//! ## Not built here: failover deprioritization
//!
//! `gateway::failover` could deprioritize a pool member whose family reports
//! `major`/`critical`. That is deliberately out of scope: a rebind driven by an
//! upstream incident must be recorded in `session_account_rebinds` with a reason
//! that names the incident, and inventing a silent routing input first is how a
//! pool starts moving sessions for reasons nobody can read back.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::Serialize;

/// Family labels the poller knows, with their summary endpoint and human page.
/// `status.anthropic.com` 301s to `status.claude.com`; the client follows
/// redirects, so either spelling works — the canonical one is used directly.
const SOURCES: &[(&str, &str, &str)] = &[
    ("anthropic", "https://status.claude.com/api/v2/summary.json", "https://status.claude.com"),
    ("openai", "https://status.openai.com/api/v2/summary.json", "https://status.openai.com"),
];

const INITIAL_DELAY: std::time::Duration = std::time::Duration::from_secs(10);
const INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

/// Normalized severity of an upstream family. `Unknown` is a first-class state:
/// no reading yet, or a reading we could not parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DegradedComponent {
    pub name: String,
    /// Statuspage component status (`degraded_performance`, `major_outage`, …).
    pub status: String,
}

/// One unresolved incident, as much of it as a badge can use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct IncidentSummary {
    pub name: String,
    /// `none` | `minor` | `major` | `critical`.
    pub impact: String,
    /// Short public link to the incident, when the page gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "string | null", optional))]
    pub url: Option<String>,
}

/// What a client gets for one family. Serializable and cheap to clone: it is
/// embedded in every usage row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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
    #[serde(skip_serializing_if = "Option::is_none")]
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

struct Slot {
    status: ProviderStatus,
    etag: Option<String>,
}

/// Per-family cache, written only by the poller.
#[derive(Default)]
pub struct ProviderStatusCache {
    slots: DashMap<&'static str, Slot>,
}

impl ProviderStatusCache {
    #[must_use]
    pub fn shared() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The reading for one family; `Unknown` when the poller has none.
    #[must_use]
    pub fn get(&self, family: &str) -> ProviderStatus {
        self.slots
            .get(family)
            .map(|s| s.status.clone())
            .unwrap_or_else(|| ProviderStatus::unknown(family))
    }

    /// Every polled family, in a stable order.
    #[must_use]
    pub fn all(&self) -> Vec<ProviderStatus> {
        SOURCES.iter().map(|(family, ..)| self.get(family)).collect()
    }

    /// The reading for an account's provider, and only when it is worth showing:
    /// a healthy or unknown family adds nothing to an account row.
    #[must_use]
    pub fn degraded_for_provider(&self, provider: &str) -> Option<ProviderStatus> {
        let family = family_of_provider(provider)?;
        let status = self.get(family);
        status.indicator.is_degraded().then_some(status)
    }

    fn record(&self, family: &'static str, status: ProviderStatus, etag: Option<String>) {
        self.slots.insert(family, Slot { status, etag });
    }

    /// A 304: the last reading still stands, only its freshness moved.
    fn touch(&self, family: &'static str) {
        if let Some(mut slot) = self.slots.get_mut(family) {
            slot.status.checked_at = Some(Utc::now());
        }
    }

    fn etag_of(&self, family: &str) -> Option<String> {
        self.slots.get(family).and_then(|s| s.etag.clone())
    }
}

/// Statuspage `summary.json` → one normalized reading.
///
/// Every field is optional in practice: a page that renames or drops one yields
/// a reading with that part missing, never a failure. An unparseable
/// `status.indicator` lands on `Unknown`, which is the honest answer.
#[must_use]
pub fn normalize(family: &str, summary: &serde_json::Value) -> ProviderStatus {
    let status = summary.get("status");
    let indicator = status
        .and_then(|s| s.get("indicator"))
        .and_then(serde_json::Value::as_str)
        .map_or(Indicator::Unknown, Indicator::parse);
    let description = status
        .and_then(|s| s.get("description"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    let url = summary
        .get("page")
        .and_then(|p| p.get("url"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .filter(|u| !u.is_empty())
        .unwrap_or_else(|| page_url(family).to_owned());

    let components = summary
        .get("components")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    let status =
                        c.get("status").and_then(serde_json::Value::as_str).unwrap_or_default();
                    if status.is_empty() || status == "operational" {
                        return None;
                    }
                    // A component group repeats its children's worst status;
                    // listing both says the same thing twice.
                    if c.get("group").and_then(serde_json::Value::as_bool).unwrap_or(false) {
                        return None;
                    }
                    let name = c.get("name").and_then(serde_json::Value::as_str)?.trim();
                    (!name.is_empty()).then(|| DegradedComponent {
                        name: name.to_owned(),
                        status: status.to_owned(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let incidents = summary
        .get("incidents")
        .and_then(serde_json::Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|i| {
                    let name = i.get("name").and_then(serde_json::Value::as_str)?.trim();
                    (!name.is_empty()).then(|| IncidentSummary {
                        name: name.to_owned(),
                        impact: i
                            .get("impact")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or("none")
                            .to_owned(),
                        url: i
                            .get("shortlink")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                            .filter(|s| !s.is_empty()),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    ProviderStatus {
        family: family.to_owned(),
        indicator,
        description,
        url,
        components,
        incidents,
        checked_at: Some(Utc::now()),
    }
}

/// `CCTUI_PROVIDER_STATUS` — enabled unless explicitly turned off, so an
/// air-gapped deployment simply reports `unknown` forever.
#[must_use]
pub fn enabled_from_env() -> bool {
    !matches!(
        std::env::var("CCTUI_PROVIDER_STATUS")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("0" | "false" | "off" | "no")
    )
}

/// Long-running poller; spawn once per process.
pub async fn task(cache: Arc<ProviderStatusCache>, http: reqwest::Client) {
    tokio::time::sleep(INITIAL_DELAY).await;
    loop {
        for (family, url, _) in SOURCES {
            poll_once(&cache, &http, family, url).await;
        }
        tokio::time::sleep(INTERVAL).await;
    }
}

/// One family, one request. Swallows every failure: a status page is never
/// allowed to matter that much.
async fn poll_once(
    cache: &ProviderStatusCache,
    http: &reqwest::Client,
    family: &'static str,
    url: &str,
) {
    let mut req = http.get(url);
    if let Some(etag) = cache.etag_of(family) {
        req = req.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(family, "provider status fetch failed: {e}");
            return;
        }
    };
    if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
        cache.touch(family);
        return;
    }
    if !resp.status().is_success() {
        tracing::debug!(family, status = %resp.status(), "provider status fetch rejected");
        return;
    }
    let etag =
        resp.headers().get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let summary: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            tracing::debug!(family, "provider status decode failed: {e}");
            return;
        }
    };
    let status = normalize(family, &summary);
    if status.indicator.is_degraded() {
        tracing::info!(family, indicator = ?status.indicator, description = %status.description, "upstream provider degraded");
    }
    cache.record(family, status, etag);
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: &str = include_str!("fixtures/provider_status/anthropic_none.json");
    const MINOR: &str = include_str!("fixtures/provider_status/anthropic_minor.json");
    const MAJOR: &str = include_str!("fixtures/provider_status/openai_major.json");
    const CRITICAL: &str = include_str!("fixtures/provider_status/openai_critical.json");

    fn parse(raw: &str) -> serde_json::Value {
        serde_json::from_str(raw).expect("fixture is valid json")
    }

    #[test]
    fn operational_summary_maps_to_none_with_no_noise() {
        let s = normalize("anthropic", &parse(NONE));
        assert_eq!(s.indicator, Indicator::None);
        assert!(!s.indicator.is_degraded());
        assert_eq!(s.description, "All Systems Operational");
        assert_eq!(s.url, "https://status.claude.com");
        assert!(s.components.is_empty(), "operational components are not listed");
        assert!(s.incidents.is_empty());
    }

    #[test]
    fn minor_summary_names_the_degraded_component_and_incident() {
        let s = normalize("anthropic", &parse(MINOR));
        assert_eq!(s.indicator, Indicator::Minor);
        assert_eq!(
            s.components,
            vec![DegradedComponent {
                name: "Claude API (api.anthropic.com)".to_owned(),
                status: "degraded_performance".to_owned(),
            }]
        );
        assert_eq!(s.incidents.len(), 1);
        assert_eq!(s.incidents[0].impact, "minor");
        assert_eq!(s.incidents[0].url.as_deref(), Some("https://stspg.io/abc123"));
    }

    #[test]
    fn major_summary_lists_every_non_operational_component() {
        let s = normalize("openai", &parse(MAJOR));
        assert_eq!(s.indicator, Indicator::Major);
        let names: Vec<&str> = s.components.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["Responses", "Login"]);
        assert!(s.indicator.is_degraded());
    }

    #[test]
    fn critical_summary_maps_to_critical() {
        let s = normalize("openai", &parse(CRITICAL));
        assert_eq!(s.indicator, Indicator::Critical);
        assert_eq!(s.incidents.len(), 1);
        assert_eq!(s.incidents[0].impact, "critical");
    }

    #[test]
    fn component_groups_are_not_repeated_as_components() {
        let summary = serde_json::json!({
            "status": {"indicator": "major", "description": "Partial outage"},
            "components": [
                {"name": "API", "status": "major_outage", "group": true},
                {"name": "Responses", "status": "major_outage", "group": false}
            ]
        });
        let s = normalize("openai", &summary);
        assert_eq!(s.components.len(), 1);
        assert_eq!(s.components[0].name, "Responses");
    }

    #[test]
    fn an_unrecognized_indicator_is_unknown_not_healthy() {
        let summary = serde_json::json!({"status": {"indicator": "sideways"}});
        assert_eq!(normalize("anthropic", &summary).indicator, Indicator::Unknown);
        let empty = serde_json::json!({});
        let s = normalize("anthropic", &empty);
        assert_eq!(s.indicator, Indicator::Unknown);
        // The page url still comes from the family, so a badge can link out.
        assert_eq!(s.url, "https://status.claude.com");
    }

    #[test]
    fn an_empty_cache_answers_unknown_for_every_family() {
        let cache = ProviderStatusCache::default();
        let all = cache.all();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|s| s.indicator == Indicator::Unknown));
        assert!(all.iter().all(|s| s.checked_at.is_none()));
        assert_eq!(cache.get("anthropic").url, "https://status.claude.com");
        assert!(cache.degraded_for_provider("anthropic").is_none());
    }

    #[test]
    fn only_first_party_providers_inherit_a_family_incident() {
        let cache = ProviderStatusCache::default();
        cache.record("anthropic", normalize("anthropic", &parse(MINOR)), None);
        assert!(cache.degraded_for_provider("anthropic").is_some());
        assert!(cache.degraded_for_provider("anthropic-compatible").is_none());
        assert!(cache.degraded_for_provider("openai-compatible").is_none());
        assert!(cache.degraded_for_provider("fireworks").is_none());
        // A healthy family is not a badge either.
        cache.record("anthropic", normalize("anthropic", &parse(NONE)), None);
        assert!(cache.degraded_for_provider("anthropic").is_none());
    }

    #[test]
    fn a_304_keeps_the_reading_and_only_moves_its_freshness() {
        let cache = ProviderStatusCache::default();
        let mut first = normalize("openai", &parse(MAJOR));
        first.checked_at = Some(DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z").unwrap().into());
        cache.record("openai", first.clone(), Some("\"abc\"".to_owned()));
        cache.touch("openai");
        let after = cache.get("openai");
        assert_eq!(after.indicator, first.indicator);
        assert_eq!(after.components, first.components);
        assert!(after.checked_at > first.checked_at);
        assert_eq!(cache.etag_of("openai").as_deref(), Some("\"abc\""));
    }

    #[test]
    fn touching_a_family_with_no_reading_is_a_no_op() {
        let cache = ProviderStatusCache::default();
        cache.touch("anthropic");
        assert_eq!(cache.get("anthropic").indicator, Indicator::Unknown);
    }

    #[test]
    fn every_source_has_a_page_url() {
        for (family, summary, page) in SOURCES {
            assert!(summary.ends_with("/api/v2/summary.json"), "{family}");
            assert_eq!(page_url(family), *page);
        }
        assert_eq!(page_url("nope"), "");
    }
}
