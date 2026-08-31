//! Per-account soft limits on the subscription usage windows.
//!
//! A cctui account (Anthropic OAuth subscription) is often shared with the user's
//! own interactive Claude Code and other workloads. Left unchecked, cctui's own
//! dispatched sessions can drive a window to 100% and rate-limit the human. The
//! soft limit caps cctui's *own* share of each window, backing off before it eats
//! the whole budget — while bypassing the cap for a window that is about to reset
//! anyway (no point hoarding it).
//!
//! Anthropic reports a self-describing `limits` array (session / weekly-all-models
//! / per-model weekly caps, and whatever it adds next), so this module treats
//! usage as a *collection* of normalized windows keyed by a stable canonical
//! identity, and lets each window carry its own independently editable cap +
//! bypass. This module is the pure decision helper: it normalizes the raw usage
//! JSON (three shapes), and evaluates a per-key cap map against it. It adds NO
//! upstream fetch and fails open for any key whose window is missing.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

/// Canonical key for the 5h / session window.
pub const KEY_SESSION: &str = "session";
/// Canonical key for the weekly all-models window.
pub const KEY_WEEKLY_ALL: &str = "weekly_all";
/// Prefix for a per-model weekly window: `weekly_model:<stable-id-or-slug>`.
pub const WEEKLY_MODEL_PREFIX: &str = "weekly_model:";
/// Per-session dollar budget (pay-per-token providers). Never resets.
pub const KEY_SESSION_USD: &str = "session_usd";
/// Rolling 5h dollar spend.
pub const KEY_USD_5H: &str = "usd_5h";
/// Rolling 7d dollar spend.
pub const KEY_USD_7D: &str = "usd_7d";

/// `Retry-After` for a blocking window with no known reset (a session budget
/// never resets): a bounded hint, not `i64::MAX`.
const NO_RESET_RETRY_SECS: i64 = 3600;

/// Whether a canonical key denotes a dollar-denominated window.
pub fn is_usd_key(key: &str) -> bool {
    matches!(key, KEY_SESSION_USD | KEY_USD_5H | KEY_USD_7D)
}

/// One window's independently editable soft-limit config. All fields optional:
/// no `cap_pct`/`cap_usd` ⇒ no cap on that window; `bypass_minutes` `None` ⇒ no
/// bypass. `cap_usd` applies to the dollar windows, `cap_pct` to the percent
/// ones; a window is evaluated against whichever its usage reports.
#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SoftLimit {
    /// Max % of the window cctui will consume before refusing more inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_pct: Option<i32>,
    /// Max USD cctui will spend in the window before refusing more inference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_usd: Option<f64>,
    /// If the window's `resets_at` is within this many minutes, ignore its cap.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bypass_minutes: Option<i32>,
}

impl SoftLimit {
    const fn is_empty(&self) -> bool {
        self.cap_pct.is_none() && self.cap_usd.is_none() && self.bypass_minutes.is_none()
    }
}

/// Per-account soft-limit configuration: a map from canonical window key to that
/// window's cap + bypass. Persisted as a validated JSONB map on the provider
/// credential, so newly discovered model-scoped windows need NO migration.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct SoftLimits {
    pub limits: BTreeMap<String, SoftLimit>,
}

impl SoftLimits {
    /// No window has a cap configured ⇒ nothing to evaluate (fast path). A bypass
    /// without a cap is inert, so it does not count as "set".
    pub fn is_unset(&self) -> bool {
        !self.limits.values().any(|l| l.cap_pct.is_some() || l.cap_usd.is_some())
    }

    /// Parse a stored JSONB soft-limit map. Unknown/malformed keys or entries are
    /// dropped (best-effort); an absent/`null` blob ⇒ empty config.
    pub fn from_json(value: Option<&serde_json::Value>) -> Self {
        let Some(obj) = value.and_then(serde_json::Value::as_object) else {
            return Self::default();
        };
        let mut limits = BTreeMap::new();
        for (key, v) in obj {
            let Some(canon) = canonicalize_key(key) else { continue };
            let limit = SoftLimit {
                cap_pct: v.get("cap_pct").and_then(serde_json::Value::as_i64).map(|n| n as i32),
                cap_usd: v
                    .get("cap_usd")
                    .and_then(serde_json::Value::as_f64)
                    .filter(|n| n.is_finite() && *n >= 0.0),
                bypass_minutes: v
                    .get("bypass_minutes")
                    .and_then(serde_json::Value::as_i64)
                    .map(|n| n as i32),
            };
            if !limit.is_empty() {
                limits.insert(canon, limit);
            }
        }
        Self { limits }
    }
}

/// A canonical window key is one of: `session`, `weekly_all`, or
/// `weekly_model:<slug>` where `<slug>` is `[a-z0-9._-]+`. Anything else is
/// rejected so an upstream label cannot inject markup or collide with another
/// account's config. Returns the normalized key (model slug re-slugged).
pub fn canonicalize_key(key: &str) -> Option<String> {
    let key = key.trim();
    if key == KEY_SESSION || key == KEY_WEEKLY_ALL || is_usd_key(key) {
        return Some(key.to_owned());
    }
    let suffix = key.strip_prefix(WEEKLY_MODEL_PREFIX)?;
    let slug = slug(suffix);
    (!slug.is_empty()).then(|| format!("{WEEKLY_MODEL_PREFIX}{slug}"))
}

/// Lowercase + collapse any run of non-`[a-z0-9._-]` characters to a single `-`,
/// trimming leading/trailing separators. Stable and markup-free. Shared with
/// `account_pick`, which slugs a requested model id the same way to match it
/// against a scoped window's key.
pub fn slug(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_dash = false;
    for c in s.trim().chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
            out.push(c);
            prev_dash = false;
        } else if !prev_dash && !out.is_empty() {
            out.push('-');
            prev_dash = true;
        }
    }
    out.trim_matches('-').to_owned()
}

/// Strip markup-ish characters and clamp a display label so an upstream-supplied
/// name can never inject markup or blow up the UI. Text only; UIs escape anyway.
fn sanitize_label(s: &str) -> String {
    let cleaned: String =
        s.chars().filter(|c| !c.is_control() && *c != '<' && *c != '>').take(64).collect();
    cleaned.trim().to_owned()
}

/// One normalized usage window, provider-agnostic and self-describing.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct UsageWindow {
    /// Stable canonical identity (`session` / `weekly_all` / `weekly_model:<id>`).
    pub key: String,
    /// Forward-compatible kind: `session` | `weekly_all` | `weekly_scoped` | other.
    pub kind: String,
    /// Human display label (`5h`, `Weekly (all models)`, `Weekly Fable`, …).
    pub label: String,
    /// Utilization percent (0–100, may exceed on overage). `0` for a dollar
    /// window with no cap to measure against.
    pub utilization: f64,
    /// USD spent in the window; set only for the dollar windows.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount_usd: Option<f64>,
    /// When the window resets (rfc3339 upstream), if known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<DateTime<Utc>>,
    /// Stable upstream model id for a scoped window, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    /// Model display name for a scoped window, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_display_name: Option<String>,
}

fn parse_resets_at(v: &serde_json::Value) -> Option<DateTime<Utc>> {
    v.get("resets_at")
        .and_then(serde_json::Value::as_str)
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&Utc))
}

/// Percent for a window, tolerating both the new `percent` and the legacy
/// `utilization` field names.
fn parse_percent(v: &serde_json::Value) -> Option<f64> {
    v.get("percent")
        .and_then(serde_json::Value::as_f64)
        .or_else(|| v.get("utilization").and_then(serde_json::Value::as_f64))
}

/// Normalize any of the three supported usage payloads into a provider-agnostic
/// collection of windows:
///   1. New Anthropic `{"limits":[{kind,percent,resets_at,scope?}, …]}`.
///   2. Legacy Anthropic fixed fields (`five_hour`/`seven_day`/`seven_day_opus`/…).
///   3. `OpenAI`'s canonical `{five_hour, seven_day}` shape (same as legacy).
///
/// Missing/malformed entries omit only themselves — one unknown limit never
/// collapses the valid ones.
pub fn normalize_usage_windows(usage: &serde_json::Value) -> Vec<UsageWindow> {
    if let Some(arr) = usage.get("limits").and_then(serde_json::Value::as_array) {
        return arr.iter().filter_map(normalize_structured_limit).collect();
    }
    normalize_fixed_fields(usage)
}

/// One entry of the new `limits[]` array → a window (or `None` if malformed).
fn normalize_structured_limit(entry: &serde_json::Value) -> Option<UsageWindow> {
    let utilization = parse_percent(entry)?;
    let resets_at = parse_resets_at(entry);
    let kind = entry.get("kind").and_then(serde_json::Value::as_str).unwrap_or("");
    let model = entry.get("scope").and_then(|s| s.get("model"));
    let model_id = model
        .and_then(|m| m.get("id"))
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .filter(|s| !s.is_empty());
    let model_display_name = model
        .and_then(|m| m.get("display_name"))
        .and_then(serde_json::Value::as_str)
        .map(sanitize_label)
        .filter(|s| !s.is_empty());

    match kind {
        "session" => Some(UsageWindow {
            key: KEY_SESSION.to_owned(),
            kind: "session".to_owned(),
            label: "5h".to_owned(),
            utilization,
            amount_usd: None,
            resets_at,
            model_id: None,
            model_display_name: None,
        }),
        "weekly_all" => Some(UsageWindow {
            key: KEY_WEEKLY_ALL.to_owned(),
            kind: "weekly_all".to_owned(),
            label: "Weekly (all models)".to_owned(),
            utilization,
            amount_usd: None,
            resets_at,
            model_id: None,
            model_display_name: None,
        }),
        // `weekly_scoped` and any future scoped kind: key off the stable model id
        // when present, else the slugged display name, so a display-name change
        // never loses config while an id exists.
        _ => {
            let slug_src = model_id.as_deref().or(model_display_name.as_deref())?;
            let s = slug(slug_src);
            if s.is_empty() {
                return None;
            }
            let label = model_display_name
                .clone()
                .map_or_else(|| format!("Weekly {s}"), |n| format!("Weekly {n}"));
            Some(UsageWindow {
                key: format!("{WEEKLY_MODEL_PREFIX}{s}"),
                kind: if kind.is_empty() { "weekly_scoped".to_owned() } else { kind.to_owned() },
                label: sanitize_label(&label),
                utilization,
                amount_usd: None,
                resets_at,
                model_id,
                model_display_name,
            })
        }
    }
}

/// Legacy/OpenAI fixed-field shape → windows.
fn normalize_fixed_fields(usage: &serde_json::Value) -> Vec<UsageWindow> {
    let mut out = Vec::new();
    let mut push = |field: &str, key: String, kind: &str, label: &str, model: Option<&str>| {
        if let Some(w) = usage.get(field)
            && let Some(utilization) = parse_percent(w)
        {
            out.push(UsageWindow {
                key,
                kind: kind.to_owned(),
                label: label.to_owned(),
                utilization,
                amount_usd: None,
                resets_at: parse_resets_at(w),
                model_id: model.map(str::to_owned),
                model_display_name: None,
            });
        }
    };
    push("five_hour", KEY_SESSION.to_owned(), "session", "5h", None);
    push("seven_day", KEY_WEEKLY_ALL.to_owned(), "weekly_all", "Weekly (all models)", None);
    push(
        "seven_day_opus",
        format!("{WEEKLY_MODEL_PREFIX}opus"),
        "weekly_scoped",
        "Weekly Opus",
        Some("opus"),
    );
    push(
        "seven_day_sonnet",
        format!("{WEEKLY_MODEL_PREFIX}sonnet"),
        "weekly_scoped",
        "Weekly Sonnet",
        Some("sonnet"),
    );
    for key in [KEY_SESSION_USD, KEY_USD_5H, KEY_USD_7D] {
        if let Some(w) = usage.get(key)
            && let Some(amount_usd) = w.get("amount_usd").and_then(serde_json::Value::as_f64)
        {
            out.push(usd_window(key, amount_usd, parse_resets_at(w)));
        }
    }
    out
}

/// Display label for a dollar window key.
pub fn usd_label(key: &str) -> &'static str {
    match key {
        KEY_USD_5H => "5h spend",
        KEY_USD_7D => "7d spend",
        _ => "Session spend",
    }
}

/// Build a dollar window. `utilization` stays 0 — a spend has no percentage
/// until a cap is set, and the cap lives in the config, not the usage.
pub fn usd_window(key: &str, amount_usd: f64, resets_at: Option<DateTime<Utc>>) -> UsageWindow {
    UsageWindow {
        key: key.to_owned(),
        kind: "usd".to_owned(),
        label: usd_label(key).to_owned(),
        utilization: 0.0,
        amount_usd: Some(amount_usd),
        resets_at,
        model_id: None,
        model_display_name: None,
    }
}

/// Outcome of evaluating an account's usage against its soft limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Under cap, within a bypass window, no cap set, or no usage data — proxy.
    Allow,
    /// At/over a cap and not within the bypass window — refuse with a reason.
    Block {
        /// Seconds until the nearest blocking window resets (for `Retry-After`).
        retry_after_secs: i64,
        /// Human-readable reason surfaced to the worker/UI in the 429 body.
        reason: String,
        /// Canonical key of the blocking window (identifies a model-scoped block).
        key: String,
    },
}

/// Decide whether to allow an inference request, given the normalized usage
/// windows and the per-key cap map.
///
/// Each configured window is evaluated INDEPENDENTLY against its matching
/// normalized window. Fails open: no caps, or a configured key with no matching
/// window, or no usage ⇒ `Allow` (for that key). A key blocks only when its
/// utilization is at/above its cap AND its reset is more than its own
/// `bypass_minutes` away (or unknown). When several keys block, the reason names
/// the nearest-resetting one and `retry_after` is derived from that reset.
pub fn evaluate_soft_limit(
    windows: &[UsageWindow],
    caps: &SoftLimits,
    now: DateTime<Utc>,
) -> Decision {
    if caps.is_unset() {
        return Decision::Allow;
    }

    let mut blocking: Vec<(i64, String, String)> = Vec::new();
    for (key, limit) in &caps.limits {
        // Missing window for a configured key ⇒ fail open for that key only.
        let Some(win) = windows.iter().find(|w| &w.key == key) else { continue };
        let over = match (limit.cap_usd, win.amount_usd, limit.cap_pct) {
            (Some(cap_usd), Some(spent), _) => (spent >= cap_usd)
                .then(|| format!("{} at ${spent:.2} (cap ${cap_usd:.2})", win.label)),
            (_, _, Some(cap)) => (win.utilization >= f64::from(cap)).then(|| {
                format!("{} window at {}% (cap {cap}%)", win.label, win.utilization.round() as i64)
            }),
            _ => None,
        };
        let Some(detail) = over else { continue };
        // A window that never resets can never be "about to reset": no bypass.
        let Some(resets_at) = win.resets_at else {
            blocking.push((
                NO_RESET_RETRY_SECS,
                format!("cctui soft limit: {detail}"),
                key.clone(),
            ));
            continue;
        };
        let bypass = i64::from(limit.bypass_minutes.unwrap_or(0).max(0));
        let secs_to_reset = (resets_at - now).num_seconds();
        if secs_to_reset > 0 && secs_to_reset <= bypass * 60 {
            continue;
        }
        let retry = secs_to_reset.max(1);
        let mins = (retry + 59) / 60;
        blocking.push((
            retry,
            format!("cctui soft limit: {detail}, resets in {mins}m"),
            key.clone(),
        ));
    }

    match blocking.into_iter().min_by_key(|(secs, _, _)| *secs) {
        Some((retry_after_secs, reason, key)) => Decision::Block { retry_after_secs, reason, key },
        None => Decision::Allow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-06-19T12:00:00Z").unwrap().with_timezone(&Utc)
    }

    fn caps(pairs: &[(&str, Option<i32>, Option<i32>)]) -> SoftLimits {
        let mut limits = BTreeMap::new();
        for (k, cap, bypass) in pairs {
            limits.insert(
                (*k).to_owned(),
                SoftLimit { cap_pct: *cap, cap_usd: None, bypass_minutes: *bypass },
            );
        }
        SoftLimits { limits }
    }

    fn usd_caps(pairs: &[(&str, f64, Option<i32>)]) -> SoftLimits {
        let mut limits = BTreeMap::new();
        for (k, cap, bypass) in pairs {
            limits.insert(
                (*k).to_owned(),
                SoftLimit { cap_pct: None, cap_usd: Some(*cap), bypass_minutes: *bypass },
            );
        }
        SoftLimits { limits }
    }

    fn legacy(five: f64, five_reset: &str, seven: f64, seven_reset: &str) -> serde_json::Value {
        json!({
            "five_hour": { "utilization": five, "resets_at": five_reset },
            "seven_day": { "utilization": seven, "resets_at": seven_reset },
        })
    }

    // ---- normalization -----------------------------------------------------

    #[test]
    fn structured_limits_render_all_windows() {
        // Acceptance (1): 5h 3%, weekly-all 82%, weekly Fable 100%.
        let payload = json!({"limits": [
            {"kind":"session","percent":3,"resets_at":"2026-06-19T16:00:00Z"},
            {"kind":"weekly_all","percent":82,"resets_at":"2026-06-26T00:00:00Z"},
            {"kind":"weekly_scoped","percent":100,"resets_at":"2026-06-26T00:00:00Z",
             "scope":{"model":{"id":null,"display_name":"Fable"}}},
        ]});
        let w = normalize_usage_windows(&payload);
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].key, "session");
        assert!((w[0].utilization - 3.0).abs() < 1e-9);
        assert_eq!(w[1].key, "weekly_all");
        assert!((w[1].utilization - 82.0).abs() < 1e-9);
        assert_eq!(w[2].key, "weekly_model:fable");
        assert_eq!(w[2].label, "Weekly Fable");
        assert!((w[2].utilization - 100.0).abs() < 1e-9);
        assert_eq!(w[2].model_display_name.as_deref(), Some("Fable"));
    }

    #[test]
    fn scoped_key_prefers_stable_id_over_display_name() {
        // Acceptance (6-ish): a display-name change must not move the key when a
        // stable id exists.
        let a = json!({"limits":[{"kind":"weekly_scoped","percent":50,
            "scope":{"model":{"id":"claude-opus-4-8","display_name":"Opus 4.8"}}}]});
        let b = json!({"limits":[{"kind":"weekly_scoped","percent":50,
            "scope":{"model":{"id":"claude-opus-4-8","display_name":"Opus (renamed)"}}}]});
        assert_eq!(normalize_usage_windows(&a)[0].key, normalize_usage_windows(&b)[0].key);
        assert_eq!(normalize_usage_windows(&a)[0].key, "weekly_model:claude-opus-4-8");
    }

    #[test]
    fn dynamic_scoped_model_appears_without_hardcoding() {
        // Acceptance (3): a never-before-seen model name normalizes fine.
        let payload = json!({"limits":[{"kind":"weekly_scoped","percent":40,
            "scope":{"model":{"id":null,"display_name":"Nebula-9"}}}]});
        let w = normalize_usage_windows(&payload);
        assert_eq!(w[0].key, "weekly_model:nebula-9");
        assert_eq!(w[0].label, "Weekly Nebula-9");
    }

    #[test]
    fn malformed_entry_omits_only_itself() {
        // Acceptance (8): one null/garbage entry doesn't hide the valid ones.
        let payload = json!({"limits":[
            {"kind":"session","percent":3,"resets_at":"2026-06-19T16:00:00Z"},
            {"kind":"weekly_scoped","scope":{"model":{"id":null,"display_name":"NoPercent"}}},
            serde_json::Value::Null,
            {"kind":"weekly_all","percent":50},
        ]});
        let w = normalize_usage_windows(&payload);
        let keys: Vec<_> = w.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["session", "weekly_all"]);
    }

    #[test]
    fn legacy_and_openai_fixed_fields_normalize() {
        // Acceptance (5): legacy Anthropic / OpenAI shapes still produce windows.
        let payload = json!({
            "five_hour": {"utilization": 12.0, "resets_at": "2026-06-19T16:00:00Z"},
            "seven_day": {"utilization": 34.0, "resets_at": "2026-06-26T00:00:00Z"},
            "seven_day_opus": {"utilization": 56.0, "resets_at": "2026-06-26T00:00:00Z"},
        });
        let w = normalize_usage_windows(&payload);
        let keys: Vec<_> = w.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["session", "weekly_all", "weekly_model:opus"]);
        assert_eq!(w[2].label, "Weekly Opus");
    }

    #[test]
    fn weekly_only_response_still_yields_a_window() {
        // Acceptance (4): a weekly-only payload must not be "no usage".
        let payload = json!({"limits":[{"kind":"weekly_all","percent":70,
            "resets_at":"2026-06-26T00:00:00Z"}]});
        assert_eq!(normalize_usage_windows(&payload).len(), 1);
    }

    #[test]
    fn label_injection_is_stripped() {
        let payload = json!({"limits":[{"kind":"weekly_scoped","percent":40,
            "scope":{"model":{"id":null,"display_name":"<script>x</script>"}}}]});
        let w = normalize_usage_windows(&payload);
        assert!(!w[0].label.contains('<'));
        assert!(!w[0].key.contains('<'));
    }

    // ---- evaluation --------------------------------------------------------

    fn eval(usage: &serde_json::Value, c: &SoftLimits) -> Decision {
        evaluate_soft_limit(&normalize_usage_windows(usage), c, now())
    }

    #[test]
    fn no_caps_allows() {
        let u = legacy(99.0, "2026-06-19T16:00:00Z", 99.0, "2026-06-26T00:00:00Z");
        assert_eq!(
            evaluate_soft_limit(&normalize_usage_windows(&u), &SoftLimits::default(), now()),
            Decision::Allow
        );
    }

    #[test]
    fn missing_usage_allows() {
        let c = caps(&[(KEY_SESSION, Some(80), None)]);
        assert_eq!(evaluate_soft_limit(&[], &c, now()), Decision::Allow);
    }

    #[test]
    fn under_cap_allows() {
        let c = caps(&[(KEY_SESSION, Some(80), None)]);
        let u = legacy(50.0, "2026-06-19T16:00:00Z", 10.0, "2026-06-26T00:00:00Z");
        assert_eq!(eval(&u, &c), Decision::Allow);
    }

    #[test]
    fn over_cap_blocks_with_reason_and_retry() {
        let c = caps(&[(KEY_SESSION, Some(80), None)]);
        let u = legacy(86.0, "2026-06-19T12:41:00Z", 10.0, "2026-06-26T00:00:00Z");
        match eval(&u, &c) {
            Decision::Block { retry_after_secs, reason, key } => {
                assert_eq!(retry_after_secs, 41 * 60);
                assert_eq!(reason, "cctui soft limit: 5h window at 86% (cap 80%), resets in 41m");
                assert_eq!(key, "session");
            }
            d @ Decision::Allow => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn within_bypass_window_allows() {
        let c = caps(&[(KEY_SESSION, Some(80), Some(10))]);
        let u = legacy(95.0, "2026-06-19T12:05:00Z", 10.0, "2026-06-26T00:00:00Z");
        assert_eq!(eval(&u, &c), Decision::Allow);
    }

    #[test]
    fn per_window_independent_cap_and_bypass() {
        // Acceptance (2)/(7): session within its bypass; weekly-all blocks outside.
        let c = caps(&[(KEY_SESSION, Some(80), Some(10)), (KEY_WEEKLY_ALL, Some(70), Some(30))]);
        let u = legacy(95.0, "2026-06-19T12:05:00Z", 90.0, "2026-06-19T16:00:00Z");
        match eval(&u, &c) {
            Decision::Block { reason, key, .. } => {
                assert!(reason.contains("Weekly (all models)"), "{reason}");
                assert_eq!(key, "weekly_all");
            }
            d @ Decision::Allow => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn multi_window_retry_after_is_nearest_reset() {
        // Acceptance (7): both over cap; nearer reset wins the Retry-After.
        let c = caps(&[(KEY_SESSION, Some(80), None), (KEY_WEEKLY_ALL, Some(70), None)]);
        let u = legacy(90.0, "2026-06-19T16:00:00Z", 75.0, "2026-06-19T12:20:00Z");
        match eval(&u, &c) {
            Decision::Block { retry_after_secs, key, .. } => {
                assert_eq!(retry_after_secs, 20 * 60);
                assert_eq!(key, "weekly_all");
            }
            d @ Decision::Allow => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn model_scoped_limit_blocks_and_names_itself() {
        // Acceptance (7): a model-scoped window is the blocker and is identified.
        let c = caps(&[("weekly_model:fable", Some(90), None)]);
        let u = json!({"limits":[{"kind":"weekly_scoped","percent":100,
            "resets_at":"2026-06-20T12:00:00Z","scope":{"model":{"id":null,"display_name":"Fable"}}}]});
        match eval(&u, &c) {
            Decision::Block { key, reason, .. } => {
                assert_eq!(key, "weekly_model:fable");
                assert!(reason.contains("Weekly Fable"), "{reason}");
            }
            d @ Decision::Allow => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn configured_key_without_window_fails_open() {
        // Acceptance (8): cap on a key with no matching window ⇒ allow (fail open).
        let c = caps(&[("weekly_model:ghost", Some(10), None)]);
        let u = legacy(99.0, "2026-06-19T16:00:00Z", 99.0, "2026-06-26T00:00:00Z");
        assert_eq!(eval(&u, &c), Decision::Allow);
    }

    #[test]
    fn cap_only_on_unconfigured_window_allows() {
        let c = caps(&[(KEY_SESSION, Some(80), None)]);
        let u = legacy(10.0, "2026-06-19T16:00:00Z", 99.0, "2026-06-26T00:00:00Z");
        assert_eq!(eval(&u, &c), Decision::Allow);
    }

    // ---- dollar windows ----------------------------------------------------

    fn usd_usage(five: f64, five_reset: &str, seven: f64) -> serde_json::Value {
        json!({
            "usd_5h": {"amount_usd": five, "resets_at": five_reset},
            "usd_7d": {"amount_usd": seven, "resets_at": "2026-06-26T00:00:00Z"},
        })
    }

    #[test]
    fn usd_windows_normalize_from_amounts() {
        let w = normalize_usage_windows(&usd_usage(1.5, "2026-06-19T16:00:00Z", 12.0));
        let keys: Vec<_> = w.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["usd_5h", "usd_7d"]);
        assert_eq!(w[0].kind, "usd");
        assert_eq!(w[0].label, "5h spend");
        assert!((w[0].amount_usd.unwrap() - 1.5).abs() < 1e-9);
        assert!((w[0].utilization - 0.0).abs() < 1e-9);
    }

    #[test]
    fn session_usd_normalizes_so_a_configured_cap_is_not_reported_as_unobserved() {
        let mut usage = usd_usage(1.5, "2026-06-19T16:00:00Z", 12.0);
        usage["session_usd"] = json!({ "amount_usd": 0.75, "resets_at": null });
        let w = normalize_usage_windows(&usage);
        let keys: Vec<_> = w.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["session_usd", "usd_5h", "usd_7d"]);
        let s = &w[0];
        assert_eq!(s.kind, "usd");
        assert_eq!(s.label, "Session spend");
        assert!((s.amount_usd.unwrap() - 0.75).abs() < 1e-9);
        assert!(s.resets_at.is_none(), "a session window has no rolling reset");
    }

    #[test]
    fn usd_cap_blocks_and_names_dollars() {
        let c = usd_caps(&[(KEY_USD_5H, 1.0, None)]);
        match eval(&usd_usage(1.25, "2026-06-19T12:41:00Z", 0.0), &c) {
            Decision::Block { retry_after_secs, reason, key } => {
                assert_eq!(retry_after_secs, 41 * 60);
                assert_eq!(key, KEY_USD_5H);
                assert_eq!(
                    reason,
                    "cctui soft limit: 5h spend at $1.25 (cap $1.00), resets in 41m"
                );
            }
            d @ Decision::Allow => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn usd_under_cap_and_bypass_allow() {
        let c = usd_caps(&[(KEY_USD_5H, 5.0, None)]);
        assert_eq!(eval(&usd_usage(4.99, "2026-06-19T16:00:00Z", 0.0), &c), Decision::Allow);
        let bypassing = usd_caps(&[(KEY_USD_5H, 1.0, Some(10))]);
        assert_eq!(eval(&usd_usage(9.0, "2026-06-19T12:05:00Z", 0.0), &bypassing), Decision::Allow);
    }

    #[test]
    fn session_usd_budget_blocks_without_a_reset() {
        let c = usd_caps(&[(KEY_SESSION_USD, 2.0, Some(60))]);
        let windows = vec![usd_window(KEY_SESSION_USD, 2.0, None)];
        match evaluate_soft_limit(&windows, &c, now()) {
            Decision::Block { retry_after_secs, reason, key } => {
                assert_eq!(key, KEY_SESSION_USD);
                assert_eq!(retry_after_secs, NO_RESET_RETRY_SECS);
                assert_eq!(reason, "cctui soft limit: Session spend at $2.00 (cap $2.00)");
            }
            d @ Decision::Allow => panic!("expected block, got {d:?}"),
        }
    }

    #[test]
    fn usd_cap_without_spend_data_fails_open() {
        let c = usd_caps(&[(KEY_USD_7D, 0.5, None)]);
        assert_eq!(evaluate_soft_limit(&[], &c, now()), Decision::Allow);
    }

    #[test]
    fn usd_keys_round_trip_through_json() {
        let blob = json!({
            "session_usd": {"cap_usd": 2.5},
            "usd_5h": {"cap_usd": 1.0, "bypass_minutes": 15},
            "usd_7d": {"cap_usd": -3},
        });
        let sl = SoftLimits::from_json(Some(&blob));
        assert!(!sl.is_unset());
        assert_eq!(sl.limits["session_usd"].cap_usd, Some(2.5));
        assert!(!sl.limits.contains_key("usd_7d"));
        assert_eq!(sl, SoftLimits::from_json(Some(&serde_json::to_value(&sl).unwrap())));
    }

    // ---- persistence round-trip -------------------------------------------

    #[test]
    fn from_json_round_trips_and_rejects_bad_keys() {
        let blob = json!({
            "session": {"cap_pct": 80, "bypass_minutes": 10},
            "weekly_model:Fable 2.0": {"cap_pct": 100},
            "<bad>": {"cap_pct": 50},
            "bypass_only": {"bypass_minutes": 5},
        });
        let sl = SoftLimits::from_json(Some(&blob));
        assert!(sl.limits.contains_key("session"));
        assert!(sl.limits.contains_key("weekly_model:fable-2.0"));
        assert!(!sl.limits.keys().any(|k| k.contains('<')));
        // A bypass-only, cap-less entry is inert ⇒ is_unset stays true if it's the
        // only thing set.
        assert!(!sl.is_unset()); // session has a cap
        let round = SoftLimits::from_json(Some(&serde_json::to_value(&sl).unwrap()));
        assert_eq!(sl, round);
    }

    #[test]
    fn migrated_legacy_config_still_enforces() {
        // Acceptance (6): 5h→session, 7d→weekly_all migrate without loss.
        let migrated = json!({
            "session": {"cap_pct": 80, "bypass_minutes": 10},
            "weekly_all": {"cap_pct": 70, "bypass_minutes": 360},
        });
        let c = SoftLimits::from_json(Some(&migrated));
        let u = legacy(90.0, "2026-06-19T12:41:00Z", 10.0, "2026-06-26T00:00:00Z");
        assert!(matches!(eval(&u, &c), Decision::Block { .. }));
    }
}
