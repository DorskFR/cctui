//! Quota probes for the compatible endpoints the gateway already routes to.
//!
//! Routing to an `anthropic-compatible` / `openai-compatible` credential works
//! without any of this; what a probe buys is *measurement*. Without one such a
//! credential has no windows, so `pace`, `soft_limit` and `account_pick` all see
//! `usage_known: false` — it can neither be trusted nor excluded, and a pool
//! containing it is guessing.
//!
//! This is therefore a **registry, not a taxonomy**: `account_providers.usage_probe`
//! names one entry here, the credential stays generic, and the number of provider
//! families in the UI stays at two.
//!
//! ## The design constraint
//!
//! A probe returns [`crate::soft_limit::UsageWindow`]s under the *existing*
//! canonical keys (`session`, `weekly_all`, `weekly_model:*`, `usd_*`), so
//! everything downstream picks them up with zero changes. A probe that cannot
//! express itself in those keys must not invent a private one: extend
//! `soft_limit`'s vocabulary deliberately instead. Both probes here obey that by
//! *omitting* what they cannot say — see [`LiteLlmProbe`].
//!
//! ## Why the trait is split request/parse
//!
//! `request` + `parse` are both synchronous and pure, so a probe is
//! dyn-compatible and — more importantly — its mapping is unit-testable against
//! a captured body with no network at all. [`run`] is the single place that does
//! IO, shared by every probe.

use chrono::{DateTime, Datelike, Duration, TimeZone, Utc};

use crate::soft_limit::{KEY_USD_5H, KEY_USD_7D, UsageWindow, usd_window};

/// One probe's HTTP call, fully resolved.
pub struct ProbeRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
}

/// A named quota probe for one upstream.
pub trait UsageProbe: Send + Sync {
    /// Registry id, as stored in `account_providers.usage_probe`.
    fn id(&self) -> &'static str;
    /// Operator-facing name.
    fn label(&self) -> &'static str;
    /// The call to make. `base_url` is the credential's configured endpoint.
    fn request(&self, base_url: Option<&str>, token: &str) -> ProbeRequest;
    /// The upstream body → canonical windows. An unexpected shape yields an
    /// empty vec, which reads downstream as "no measurement" — never as a
    /// zeroed, wide-open quota.
    fn parse(&self, body: &serde_json::Value, now: DateTime<Utc>) -> Vec<UsageWindow>;
}

static OPENROUTER: OpenRouterProbe = OpenRouterProbe;
static LITELLM: LiteLlmProbe = LiteLlmProbe;

/// Every registered probe, in the order the picker lists them. A `static`, not a
/// `const`: a constant may not refer to a static.
static PROBES: &[&'static dyn UsageProbe] = &[&OPENROUTER, &LITELLM];

/// Look up a probe by its stored id.
#[must_use]
pub fn probe(id: &str) -> Option<&'static dyn UsageProbe> {
    PROBES.iter().copied().find(|p| p.id() == id)
}

/// Registered probe ids — the accepted values of `usage_probe`.
#[must_use]
pub fn ids() -> Vec<&'static str> {
    PROBES.iter().map(|p| p.id()).collect()
}

/// Run a probe and return its canonical windows.
///
/// `Err` on any transport / status / decode failure, and on a body that yields
/// no windows: the caller must degrade to "usage unknown", and an `Ok(vec![])`
/// would be indistinguishable from a credential with no quota at all.
pub async fn run(
    http: &reqwest::Client,
    probe: &dyn UsageProbe,
    base_url: Option<&str>,
    token: &str,
    now: DateTime<Utc>,
) -> Result<Vec<UsageWindow>, String> {
    let req = probe.request(base_url, token);
    let mut builder = http.get(&req.url);
    for (k, v) in &req.headers {
        builder = builder.header(k, v);
    }
    let resp = builder.send().await.map_err(|e| format!("transport error: {e}"))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(format!("upstream rejected the probe: {status}"));
    }
    let body: serde_json::Value = resp.json().await.map_err(|e| format!("decode error: {e}"))?;
    let windows = probe.parse(&body, now);
    if windows.is_empty() {
        return Err("probe produced no windows".to_owned());
    }
    Ok(windows)
}

/// Canonical windows → the usage JSON shape the rest of the server already
/// consumes, so a probe's output travels the same path as an upstream payload
/// (cache, history samples, ws push, soft limit) with no special case.
///
/// Percent windows go in the self-describing `limits[]` array; dollar windows
/// stay top-level under their canonical key, which
/// [`crate::soft_limit::normalize_usage_windows`] reads in either shape.
#[must_use]
pub fn windows_to_usage_json(windows: &[UsageWindow]) -> serde_json::Value {
    let mut limits = Vec::new();
    let mut out = serde_json::Map::new();
    for w in windows {
        if crate::soft_limit::is_usd_key(&w.key) {
            let mut entry = serde_json::Map::new();
            entry.insert("amount_usd".into(), serde_json::json!(w.amount_usd.unwrap_or(0.0)));
            if let Some(at) = w.resets_at {
                entry.insert("resets_at".into(), serde_json::json!(at.to_rfc3339()));
            }
            out.insert(w.key.clone(), serde_json::Value::Object(entry));
            continue;
        }
        let mut entry = serde_json::Map::new();
        entry.insert("kind".into(), serde_json::json!(w.kind));
        entry.insert("percent".into(), serde_json::json!(w.utilization));
        if let Some(at) = w.resets_at {
            entry.insert("resets_at".into(), serde_json::json!(at.to_rfc3339()));
        }
        if w.model_id.is_some() || w.model_display_name.is_some() {
            let mut model = serde_json::Map::new();
            if let Some(id) = &w.model_id {
                model.insert("id".into(), serde_json::json!(id));
            }
            if let Some(name) = &w.model_display_name {
                model.insert("display_name".into(), serde_json::json!(name));
            }
            entry.insert("scope".into(), serde_json::json!({ "model": model }));
        }
        limits.push(serde_json::Value::Object(entry));
    }
    if !limits.is_empty() {
        out.insert("limits".into(), serde_json::Value::Array(limits));
    }
    serde_json::Value::Object(out)
}

/// Strip trailing slashes so a base URL concatenates predictably.
fn trim_base(base: &str) -> &str {
    base.trim().trim_end_matches('/')
}

/// `OpenRouter`: `GET /api/v1/key` reports the key's credit usage, including the
/// current-UTC-week total, which is exactly the `usd_7d` window.
///
/// The all-time `usage` and the lifetime `limit` are deliberately dropped: a
/// credit balance is not a rolling window, and reporting it as one would make
/// `pace` project a burn against a budget that never resets.
pub struct OpenRouterProbe;

const OPENROUTER_BASE: &str = "https://openrouter.ai/api/v1";

impl UsageProbe for OpenRouterProbe {
    fn id(&self) -> &'static str {
        "openrouter"
    }

    fn label(&self) -> &'static str {
        "OpenRouter (credits)"
    }

    fn request(&self, base_url: Option<&str>, token: &str) -> ProbeRequest {
        ProbeRequest {
            url: format!("{}/key", openrouter_api_base(base_url)),
            headers: vec![("authorization".to_owned(), format!("Bearer {token}"))],
        }
    }

    fn parse(&self, body: &serde_json::Value, now: DateTime<Utc>) -> Vec<UsageWindow> {
        let data = body.get("data").unwrap_or(body);
        let Some(weekly) = data.get("usage_weekly").and_then(serde_json::Value::as_f64) else {
            return Vec::new();
        };
        vec![usd_window(KEY_USD_7D, weekly, Some(next_utc_monday(now)))]
    }
}

/// The credential's endpoint, normalized to `…/api/v1`: an account may be
/// configured with the bare host or with the full API path, and both must reach
/// the same `key` endpoint.
fn openrouter_api_base(base_url: Option<&str>) -> String {
    let Some(base) = base_url.map(trim_base).filter(|b| !b.is_empty()) else {
        return OPENROUTER_BASE.to_owned();
    };
    if base.ends_with("/api/v1") { base.to_owned() } else { format!("{base}/api/v1") }
}

/// Start of the next UTC week (Monday 00:00), which is when `OpenRouter`'s
/// weekly counter rolls over.
fn next_utc_monday(now: DateTime<Utc>) -> DateTime<Utc> {
    let days = 7 - i64::from(now.weekday().num_days_from_monday());
    let day = (now + Duration::days(days)).date_naive();
    Utc.from_utc_datetime(&day.and_hms_opt(0, 0, 0).unwrap_or_default())
}

/// `LiteLLM`: `GET /key/info` reports a virtual key's `spend` and its budget
/// window.
///
/// Reported **only** when the key's `budget_duration` is one cctui already has a
/// canonical key for. A LiteLLM budget can be any duration (`30d`, `1mo`), and
/// there is no canonical monthly dollar window; emitting such a key as `usd_7d`
/// would make every downstream reset time a lie. Saying nothing keeps the
/// credential honestly unmeasured until the vocabulary is extended on purpose.
pub struct LiteLlmProbe;

impl UsageProbe for LiteLlmProbe {
    fn id(&self) -> &'static str {
        "litellm"
    }

    fn label(&self) -> &'static str {
        "LiteLLM (virtual-key budget)"
    }

    fn request(&self, base_url: Option<&str>, token: &str) -> ProbeRequest {
        ProbeRequest {
            url: format!("{}/key/info", litellm_root(base_url)),
            headers: vec![("authorization".to_owned(), format!("Bearer {token}"))],
        }
    }

    fn parse(&self, body: &serde_json::Value, _now: DateTime<Utc>) -> Vec<UsageWindow> {
        let info = body.get("info").unwrap_or(body);
        let Some(spend) = info.get("spend").and_then(serde_json::Value::as_f64) else {
            return Vec::new();
        };
        let Some(key) = info
            .get("budget_duration")
            .and_then(serde_json::Value::as_str)
            .and_then(usd_key_for_duration)
        else {
            return Vec::new();
        };
        let resets_at = info
            .get("budget_reset_at")
            .and_then(serde_json::Value::as_str)
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        vec![usd_window(key, spend, resets_at)]
    }
}

/// A LiteLLM `budget_duration` → the canonical dollar window of that length, or
/// `None` when cctui has no key for it.
fn usd_key_for_duration(duration: &str) -> Option<&'static str> {
    match duration.trim() {
        "5h" | "300m" => Some(KEY_USD_5H),
        "7d" | "1w" | "168h" => Some(KEY_USD_7D),
        _ => None,
    }
}

/// The LiteLLM admin root: the credential's base URL points at the OpenAI-shaped
/// `…/v1`, while the key-management endpoints hang off the root.
fn litellm_root(base_url: Option<&str>) -> String {
    let base = base_url.map(trim_base).filter(|b| !b.is_empty()).unwrap_or_default();
    base.strip_suffix("/v1").unwrap_or(base).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soft_limit::{KEY_SESSION, normalize_usage_windows};

    fn now() -> DateTime<Utc> {
        // A Wednesday.
        DateTime::parse_from_rfc3339("2026-09-09T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    #[test]
    fn the_registry_resolves_only_registered_ids() {
        assert_eq!(ids(), vec!["openrouter", "litellm"]);
        assert_eq!(probe("openrouter").map(UsageProbe::id), Some("openrouter"));
        assert_eq!(probe("litellm").map(UsageProbe::id), Some("litellm"));
        assert!(probe("zai").is_none());
        assert!(probe("").is_none());
    }

    #[test]
    fn openrouter_reports_the_weekly_spend_as_the_canonical_7d_dollar_window() {
        let body = serde_json::json!({
            "data": {
                "label": "cctui",
                "usage": 412.5,
                "usage_daily": 3.0,
                "usage_weekly": 21.75,
                "usage_monthly": 88.0,
                "limit": 100.0,
                "limit_remaining": 78.25,
                "is_free_tier": false
            }
        });
        let windows = OPENROUTER.parse(&body, now());
        assert_eq!(windows.len(), 1, "the all-time and monthly counters are not windows");
        assert_eq!(windows[0].key, KEY_USD_7D);
        assert_eq!(windows[0].amount_usd, Some(21.75));
        assert_eq!(windows[0].kind, "usd");
        // Wednesday → the following Monday, midnight UTC.
        assert_eq!(windows[0].resets_at.unwrap().to_rfc3339(), "2026-09-14T00:00:00+00:00");
    }

    #[test]
    fn openrouter_tolerates_an_unwrapped_body_and_a_missing_counter() {
        let flat = serde_json::json!({"usage_weekly": 1.5});
        assert_eq!(OPENROUTER.parse(&flat, now())[0].amount_usd, Some(1.5));
        assert!(OPENROUTER.parse(&serde_json::json!({"data": {"usage": 9.0}}), now()).is_empty());
        assert!(OPENROUTER.parse(&serde_json::json!({}), now()).is_empty());
    }

    #[test]
    fn openrouter_normalizes_whatever_base_url_the_credential_carries() {
        assert_eq!(openrouter_api_base(None), OPENROUTER_BASE);
        assert_eq!(openrouter_api_base(Some("https://openrouter.ai/api/v1")), OPENROUTER_BASE);
        assert_eq!(openrouter_api_base(Some("https://openrouter.ai/api/v1/")), OPENROUTER_BASE);
        assert_eq!(
            openrouter_api_base(Some("https://proxy.internal")),
            "https://proxy.internal/api/v1"
        );
        assert_eq!(openrouter_api_base(Some("  ")), OPENROUTER_BASE);
        assert_eq!(OPENROUTER.request(None, "sk-or-x").url, "https://openrouter.ai/api/v1/key");
        assert_eq!(
            OPENROUTER.request(None, "sk-or-x").headers,
            vec![("authorization".to_owned(), "Bearer sk-or-x".to_owned())]
        );
    }

    #[test]
    fn a_monday_rolls_over_to_the_next_monday_not_today() {
        let monday =
            DateTime::parse_from_rfc3339("2026-09-14T09:00:00Z").unwrap().with_timezone(&Utc);
        assert_eq!(next_utc_monday(monday).to_rfc3339(), "2026-09-21T00:00:00+00:00");
    }

    #[test]
    fn litellm_reports_a_budget_whose_duration_has_a_canonical_key() {
        let body = serde_json::json!({
            "key": "sk-1234",
            "info": {
                "spend": 4.25,
                "max_budget": 50.0,
                "budget_duration": "7d",
                "budget_reset_at": "2026-09-16T00:00:00Z"
            }
        });
        let windows = LITELLM.parse(&body, now());
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].key, KEY_USD_7D);
        assert_eq!(windows[0].amount_usd, Some(4.25));
        assert_eq!(windows[0].resets_at.unwrap().to_rfc3339(), "2026-09-16T00:00:00+00:00");
    }

    #[test]
    fn litellm_says_nothing_rather_than_mislabel_a_budget_it_has_no_key_for() {
        let monthly = serde_json::json!({
            "info": {"spend": 4.25, "budget_duration": "30d", "budget_reset_at": "2026-10-01T00:00:00Z"}
        });
        assert!(LITELLM.parse(&monthly, now()).is_empty());
        let unbudgeted = serde_json::json!({"info": {"spend": 4.25}});
        assert!(LITELLM.parse(&unbudgeted, now()).is_empty());
        let empty = serde_json::json!({"info": {"budget_duration": "7d"}});
        assert!(LITELLM.parse(&empty, now()).is_empty());
    }

    #[test]
    fn litellm_maps_the_durations_it_does_know() {
        assert_eq!(usd_key_for_duration("5h"), Some(KEY_USD_5H));
        assert_eq!(usd_key_for_duration(" 7d "), Some(KEY_USD_7D));
        assert_eq!(usd_key_for_duration("1w"), Some(KEY_USD_7D));
        assert_eq!(usd_key_for_duration("1mo"), None);
    }

    #[test]
    fn litellm_hangs_key_info_off_the_admin_root_not_the_openai_path() {
        assert_eq!(litellm_root(Some("https://litellm.internal/v1")), "https://litellm.internal");
        assert_eq!(litellm_root(Some("https://litellm.internal/v1/")), "https://litellm.internal");
        assert_eq!(litellm_root(Some("https://litellm.internal")), "https://litellm.internal");
        assert_eq!(
            LITELLM.request(Some("https://litellm.internal/v1"), "sk-1").url,
            "https://litellm.internal/key/info"
        );
    }

    #[test]
    fn a_dollar_window_round_trips_through_the_usage_json() {
        let windows = vec![usd_window(KEY_USD_7D, 12.5, Some(now()))];
        let json = windows_to_usage_json(&windows);
        let back = normalize_usage_windows(&json);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].key, KEY_USD_7D);
        assert_eq!(back[0].amount_usd, Some(12.5));
        assert_eq!(back[0].resets_at, Some(now()));
    }

    #[test]
    fn a_percent_window_round_trips_through_the_usage_json() {
        let windows = vec![UsageWindow {
            key: KEY_SESSION.to_owned(),
            kind: "session".to_owned(),
            label: "5h".to_owned(),
            utilization: 42.0,
            amount_usd: None,
            resets_at: Some(now()),
            model_id: None,
            model_display_name: None,
        }];
        let back = normalize_usage_windows(&windows_to_usage_json(&windows));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].key, KEY_SESSION);
        assert_eq!(back[0].utilization, 42.0);
        assert_eq!(back[0].resets_at, Some(now()));
    }

    #[test]
    fn percent_and_dollar_windows_survive_the_same_payload() {
        let windows = vec![
            UsageWindow {
                key: KEY_SESSION.to_owned(),
                kind: "session".to_owned(),
                label: "5h".to_owned(),
                utilization: 10.0,
                amount_usd: None,
                resets_at: None,
                model_id: None,
                model_display_name: None,
            },
            usd_window(KEY_USD_7D, 3.0, None),
        ];
        let back = normalize_usage_windows(&windows_to_usage_json(&windows));
        let keys: Vec<&str> = back.iter().map(|w| w.key.as_str()).collect();
        assert!(keys.contains(&KEY_SESSION), "{keys:?}");
        assert!(keys.contains(&KEY_USD_7D), "{keys:?}");
    }

    #[test]
    fn a_model_scoped_window_keeps_its_identity_through_the_payload() {
        let windows = vec![UsageWindow {
            key: "weekly_model:glm-5".to_owned(),
            kind: "weekly_scoped".to_owned(),
            label: "Weekly GLM 5".to_owned(),
            utilization: 5.0,
            amount_usd: None,
            resets_at: None,
            model_id: Some("glm-5".to_owned()),
            model_display_name: Some("GLM 5".to_owned()),
        }];
        let back = normalize_usage_windows(&windows_to_usage_json(&windows));
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].key, "weekly_model:glm-5");
    }

    #[test]
    fn no_windows_serialize_to_an_empty_object_not_a_null_payload() {
        assert_eq!(windows_to_usage_json(&[]), serde_json::json!({}));
        assert!(normalize_usage_windows(&windows_to_usage_json(&[])).is_empty());
    }
}
