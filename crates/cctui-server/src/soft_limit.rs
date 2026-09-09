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

/// Minimum elapsed share before a window's pace is enforceable: `expected_pct`
/// is ~0 at a window's start, so the first request of a fresh window would
/// otherwise read as an infinite burn. 10% is 30 minutes of a 5h window.
const PACE_MIN_ELAPSED_FRACTION: f64 = 0.10;
/// Bounds on the pace back-off — long enough to slow a burst, short enough that
/// the harness keeps making progress.
const PACE_RETRY_MIN_SECS: i64 = 30;
const PACE_RETRY_MAX_SECS: i64 = 900;
/// Prefix on a pace refusal's blocking key: a burn rate, not a spent budget.
pub const PACE_REASON_PREFIX: &str = "pace:";

/// Prefix on a durable block key whose cap came from the session's own budget
/// rather than the account configuration.
pub const SESSION_SCOPE_PREFIX: &str = "session_scope:";

/// Whether a canonical key denotes a dollar-denominated window.
pub fn is_usd_key(key: &str) -> bool {
    matches!(key, KEY_SESSION_USD | KEY_USD_5H | KEY_USD_7D)
}

/// Whether a canonical key denotes a per-model weekly window.
pub fn is_model_scoped_key(key: &str) -> bool {
    key.starts_with(WEEKLY_MODEL_PREFIX)
}

/// Whether a normalized window applies to the model a request will run.
///
/// Non-scoped windows (5h, weekly-all, the dollar ones) always apply. A scoped
/// window applies only when its model matches; with no model known we cannot
/// tell, so every window applies — the conservative side, which can only narrow
/// a margin, never overstate it. Enforcement and election share this one
/// definition so they cannot drift apart.
pub fn window_applies(window: &UsageWindow, model: Option<&str>) -> bool {
    let Some(scoped) = window.key.strip_prefix(WEEKLY_MODEL_PREFIX) else { return true };
    let Some(model) = model else { return true };
    let requested = slug(model);
    if requested.is_empty() || scoped.is_empty() {
        return true;
    }
    // Either direction: the request may be an alias the window spells out
    // (`fable` vs `claude-fable-5`) or a fuller id than the window's
    // (`claude-opus-4-8-1m` vs `claude-opus-4-8`).
    requested.contains(scoped) || scoped.contains(&requested)
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
    /// Max burn rate as a multiple of the window's linear budget: `1.5` refuses
    /// once the window is spent 50% faster than evenly. Percent windows only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace_cap: Option<f32>,
}

impl SoftLimit {
    const fn is_empty(&self) -> bool {
        self.cap_pct.is_none()
            && self.cap_usd.is_none()
            && self.bypass_minutes.is_none()
            && self.pace_cap.is_none()
    }

    fn effective_pace_cap(&self) -> Option<f64> {
        self.pace_cap.map(f64::from).filter(|c| c.is_finite() && *c > 0.0)
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
        !self
            .limits
            .values()
            .any(|l| l.cap_pct.is_some() || l.cap_usd.is_some() || l.pace_cap.is_some())
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
                pace_cap: v
                    .get("pace_cap")
                    .and_then(serde_json::Value::as_f64)
                    .filter(|n| n.is_finite() && *n > 0.0)
                    .map(|n| n as f32),
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

/// Identity from the duration upstream reported. The `five_hour`/`seven_day`
/// slots are positional, not descriptive — a weekly-only plan reports its
/// weekly limit in the primary slot. `None` keeps the slot's default identity.
fn identity_from_seconds(secs: i64) -> Option<(String, &'static str, String)> {
    if secs <= 0 {
        return None;
    }
    if secs < 86_400 {
        let hours = (secs as f64 / 3600.0).round().max(1.0) as i64;
        return Some((KEY_SESSION.to_owned(), "session", format!("{hours}h")));
    }
    let days = (secs as f64 / 86_400.0).round().max(1.0) as i64;
    let label =
        if days == 7 { "Weekly (all models)".to_owned() } else { format!("{days}d (all models)") };
    Some((KEY_WEEKLY_ALL.to_owned(), "weekly_all", label))
}

/// Legacy/OpenAI fixed-field shape → windows.
fn normalize_fixed_fields(usage: &serde_json::Value) -> Vec<UsageWindow> {
    let mut out: Vec<UsageWindow> = Vec::new();
    let mut push = |field: &str, key: String, kind: &str, label: &str, model: Option<&str>| {
        if let Some(w) = usage.get(field)
            && let Some(utilization) = parse_percent(w)
        {
            let reclassified = model
                .is_none()
                .then(|| {
                    w.get("window_seconds")
                        .and_then(serde_json::Value::as_i64)
                        .and_then(identity_from_seconds)
                })
                .flatten();
            let (key, kind, label) = match reclassified {
                Some((k, kd, l)) => (k, kd, l),
                None => (key, kind, label.to_owned()),
            };
            if out.iter().any(|existing| existing.key == key) {
                return;
            }
            out.push(UsageWindow {
                key,
                kind: kind.to_owned(),
                label,
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
///
/// `model` is the model the request will run. A `weekly_model:` window is only
/// evaluated when it applies to it (see [`window_applies`]) — a spent weekly
/// Fable budget must not block an Opus request. `None` means "not known here"
/// and keeps the conservative reading: every window counts.
pub fn evaluate_soft_limit(
    windows: &[UsageWindow],
    caps: &SoftLimits,
    model: Option<&str>,
    now: DateTime<Utc>,
) -> Decision {
    if caps.is_unset() {
        return Decision::Allow;
    }

    let mut blocking: Vec<(i64, String, String)> = Vec::new();
    for (key, limit) in &caps.limits {
        // Missing window for a configured key ⇒ fail open for that key only.
        let Some(win) = windows.iter().find(|w| &w.key == key) else { continue };
        if !window_applies(win, model) {
            continue;
        }
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

    // A level block outranks a pace block: its horizon is the real one, and a
    // pace back-off (seconds) would otherwise always win the `min` and send the
    // worker back into a wall it cannot clear until the window resets.
    if let Some((retry_after_secs, reason, key)) =
        blocking.into_iter().min_by_key(|(secs, _, _)| *secs)
    {
        return Decision::Block { retry_after_secs, reason, key };
    }
    evaluate_pace(windows, caps, model, now)
}

/// Whether a block durably recorded under `key` is lifted by an account's
/// `caps`.
///
/// Only the limit that caused the block gets a say: a session refused by its own
/// `CctuiAgent` budget carries a [`SESSION_SCOPE_PREFIX`] key and survives every
/// account-level change, and an account window is judged against its own cap
/// alone so an unrelated window still over cap cannot hold it. A key the account
/// no longer caps is lifted — the cap that produced it is gone.
pub fn block_lifted_by(
    key: &str,
    windows: &[UsageWindow],
    caps: &SoftLimits,
    now: DateTime<Utc>,
) -> bool {
    if key.starts_with(SESSION_SCOPE_PREFIX) {
        return false;
    }
    let window_key = key.strip_prefix(PACE_REASON_PREFIX).unwrap_or(key);
    let Some(limit) = caps.limits.get(window_key) else { return true };
    let scoped = SoftLimits { limits: BTreeMap::from([(window_key.to_owned(), *limit)]) };
    matches!(evaluate_soft_limit(windows, &scoped, None, now), Decision::Allow)
}

/// Refuse a window being burned faster than `pace_cap` times its linear budget.
///
/// Fails open wherever the rate is unknowable: no `pace_cap`, no matching
/// window, a window with no `resets_at` or no known length, or one too early in
/// its span to divide by (see [`PACE_MIN_ELAPSED_FRACTION`]). Dollar windows
/// report no utilization, so they never have a pace.
fn evaluate_pace(
    windows: &[UsageWindow],
    caps: &SoftLimits,
    model: Option<&str>,
    now: DateTime<Utc>,
) -> Decision {
    let mut blocking: Vec<(i64, String, String)> = Vec::new();
    for (key, limit) in &caps.limits {
        let Some(cap) = limit.effective_pace_cap() else { continue };
        let Some(win) = windows.iter().find(|w| &w.key == key) else { continue };
        if win.amount_usd.is_some() || !window_applies(win, model) {
            continue;
        }
        let Some(pace) = crate::pace::for_window(now, win, None) else { continue };
        if pace.elapsed_fraction < PACE_MIN_ELAPSED_FRACTION || pace.ratio <= cap {
            continue;
        }
        if let Some(resets_at) = win.resets_at {
            let bypass = i64::from(limit.bypass_minutes.unwrap_or(0).max(0));
            let secs_to_reset = (resets_at - now).num_seconds();
            if secs_to_reset > 0 && secs_to_reset <= bypass * 60 {
                continue;
            }
        }
        // Waiting this long re-earns the budget already spent: the elapsed span
        // grows until `expected_pct` has caught up with the current burn.
        let elapsed_secs = pace.elapsed_fraction
            * crate::pace::window_duration(&win.key).map_or(0.0, |d| d.num_seconds() as f64);
        let retry = (elapsed_secs * (pace.ratio / cap - 1.0)).round() as i64;
        blocking.push((
            retry.clamp(PACE_RETRY_MIN_SECS, PACE_RETRY_MAX_SECS),
            format!(
                "cctui pace limit: {} window at {}% with {}% expected by now ({:.1}x, cap {cap:.1}x)",
                win.label,
                win.utilization.round() as i64,
                pace.expected_pct.round() as i64,
                pace.ratio,
            ),
            format!("{PACE_REASON_PREFIX}{key}"),
        ));
    }
    match blocking.into_iter().max_by_key(|(secs, _, _)| *secs) {
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
                SoftLimit { cap_pct: *cap, cap_usd: None, bypass_minutes: *bypass, pace_cap: None },
            );
        }
        SoftLimits { limits }
    }

    fn usd_caps(pairs: &[(&str, f64, Option<i32>)]) -> SoftLimits {
        let mut limits = BTreeMap::new();
        for (k, cap, bypass) in pairs {
            limits.insert(
                (*k).to_owned(),
                SoftLimit {
                    cap_pct: None,
                    cap_usd: Some(*cap),
                    bypass_minutes: *bypass,
                    pace_cap: None,
                },
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
    fn primary_slot_carrying_a_weekly_duration_is_keyed_weekly() {
        let payload = json!({
            "five_hour": {
                "utilization": 35.0,
                "resets_at": "2026-09-11T06:02:31Z",
                "window_seconds": 604_800,
            },
        });
        let w = normalize_usage_windows(&payload);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].key, "weekly_all");
        assert_eq!(w[0].kind, "weekly_all");
        assert_eq!(w[0].label, "Weekly (all models)");
    }

    #[test]
    fn duplicate_durations_collapse_to_one_window() {
        let payload = json!({
            "five_hour": { "utilization": 35.0, "window_seconds": 604_800 },
            "seven_day": { "utilization": 40.0, "window_seconds": 604_800 },
        });
        let w = normalize_usage_windows(&payload);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].key, "weekly_all");
        assert!((w[0].utilization - 35.0).abs() < 1e-9);
    }

    #[test]
    fn windows_without_a_duration_keep_their_slot_identity() {
        let w = normalize_usage_windows(&legacy(
            90.0,
            "2026-06-19T16:00:00Z",
            10.0,
            "2026-06-26T00:00:00Z",
        ));
        assert_eq!(w[0].key, "session");
        assert_eq!(w[0].label, "5h");
        assert_eq!(w[1].key, "weekly_all");
    }

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
        eval_for(usage, c, None)
    }

    fn eval_for(usage: &serde_json::Value, c: &SoftLimits, model: Option<&str>) -> Decision {
        evaluate_soft_limit(&normalize_usage_windows(usage), c, model, now())
    }

    #[test]
    fn no_caps_allows() {
        let u = legacy(99.0, "2026-06-19T16:00:00Z", 99.0, "2026-06-26T00:00:00Z");
        assert_eq!(
            evaluate_soft_limit(&normalize_usage_windows(&u), &SoftLimits::default(), None, now()),
            Decision::Allow
        );
    }

    #[test]
    fn missing_usage_allows() {
        let c = caps(&[(KEY_SESSION, Some(80), None)]);
        assert_eq!(evaluate_soft_limit(&[], &c, None, now()), Decision::Allow);
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
        // It blocks the model it names, and requests whose model is unknown here
        // (the conservative reading), but nothing else.
        let c = caps(&[("weekly_model:fable", Some(90), None)]);
        let u = json!({"limits":[{"kind":"weekly_scoped","percent":100,
            "resets_at":"2026-06-20T12:00:00Z","scope":{"model":{"id":null,"display_name":"Fable"}}}]});
        for model in [None, Some("claude-fable-5")] {
            match eval_for(&u, &c, model) {
                Decision::Block { key, reason, .. } => {
                    assert_eq!(key, "weekly_model:fable");
                    assert!(reason.contains("Weekly Fable"), "{reason}");
                }
                d @ Decision::Allow => panic!("expected block for {model:?}, got {d:?}"),
            }
        }
    }

    #[test]
    fn a_spent_fable_budget_does_not_block_another_model() {
        let c = caps(&[("weekly_model:fable", Some(90), None)]);
        let u = json!({"limits":[{"kind":"weekly_scoped","percent":100,
            "resets_at":"2026-06-20T12:00:00Z","scope":{"model":{"id":null,"display_name":"Fable"}}}]});
        assert_eq!(
            eval_for(&u, &c, Some("claude-opus-4-8-1m")),
            Decision::Allow,
            "a spent weekly Fable budget must not refuse an Opus request"
        );
    }

    /// The gateway gates the model-blind windows before it has read the body,
    /// then the scoped ones once the model is known. The early half must fail
    /// open on a scoped cap rather than block on a window it cannot judge.
    #[test]
    fn the_model_blind_window_subset_never_enforces_a_scoped_cap() {
        let c = caps(&[("weekly_model:fable", Some(90), None)]);
        let u = json!({"limits":[
            {"kind":"session","percent":10,"resets_at":"2026-06-19T16:00:00Z"},
            {"kind":"weekly_scoped","percent":100,"resets_at":"2026-06-20T12:00:00Z",
             "scope":{"model":{"id":null,"display_name":"Fable"}}}]});
        let unscoped: Vec<UsageWindow> = normalize_usage_windows(&u)
            .into_iter()
            .filter(|w| !is_model_scoped_key(&w.key))
            .collect();
        assert_eq!(evaluate_soft_limit(&unscoped, &c, None, now()), Decision::Allow);
    }

    #[test]
    fn an_account_wide_window_blocks_every_model() {
        let c = caps(&[(KEY_WEEKLY_ALL, Some(90), None)]);
        let u = legacy(10.0, "2026-06-19T16:00:00Z", 95.0, "2026-06-26T00:00:00Z");
        for model in [None, Some("claude-fable-5"), Some("claude-opus-4-8")] {
            assert!(
                matches!(eval_for(&u, &c, model), Decision::Block { .. }),
                "the weekly-all window applies to {model:?} like any other"
            );
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

    // ---- pace --------------------------------------------------------------

    fn pace_caps(pairs: &[(&str, f32, Option<i32>)]) -> SoftLimits {
        let mut limits = BTreeMap::new();
        for (k, cap, bypass) in pairs {
            limits.insert(
                (*k).to_owned(),
                SoftLimit {
                    cap_pct: None,
                    cap_usd: None,
                    bypass_minutes: *bypass,
                    pace_cap: Some(*cap),
                },
            );
        }
        SoftLimits { limits }
    }

    /// One hour into a 5h window ⇒ 20% expected; `resets_at` is 4h out.
    fn one_hour_in(utilization: f64) -> serde_json::Value {
        json!({"limits": [{
            "kind": "session", "percent": utilization, "resets_at": "2026-06-19T16:00:00Z",
        }]})
    }

    #[test]
    fn over_pace_blocks_with_a_bounded_retry_after() {
        // Acceptance: pace_cap 1.5 on the 5h window, 60% used after 1h of 5
        // (20% expected) ⇒ ratio 3.0 ⇒ 429.
        let c = pace_caps(&[(KEY_SESSION, 1.5, None)]);
        match eval(&one_hour_in(60.0), &c) {
            Decision::Block { retry_after_secs, reason, key } => {
                assert_eq!(key, "pace:session");
                assert!(
                    (PACE_RETRY_MIN_SECS..=PACE_RETRY_MAX_SECS).contains(&retry_after_secs),
                    "retry {retry_after_secs}s must stay bounded"
                );
                assert_eq!(
                    reason,
                    "cctui pace limit: 5h window at 60% with 20% expected by now \
                     (3.0x, cap 1.5x)"
                );
            }
            d @ Decision::Allow => panic!("expected a pace block, got {d:?}"),
        }
    }

    #[test]
    fn the_block_clears_once_the_ratio_falls_under_the_cap() {
        // Same window and cap; 25% after 1h is 1.25x — under 1.5x.
        let c = pace_caps(&[(KEY_SESSION, 1.5, None)]);
        assert_eq!(eval(&one_hour_in(25.0), &c), Decision::Allow);
    }

    #[test]
    fn a_window_barely_started_makes_no_pace_decision() {
        // 6 minutes into 5h is 2% elapsed: below the enforceable floor, so even
        // a wild ratio must fail open rather than refuse the window's first call.
        let c = pace_caps(&[(KEY_SESSION, 1.5, None)]);
        let u = json!({"limits":[{"kind":"session","percent":30.0,
            "resets_at":"2026-06-19T16:54:00Z"}]});
        assert_eq!(eval(&u, &c), Decision::Allow);
    }

    #[test]
    fn a_window_without_a_reset_makes_no_pace_decision() {
        let c = pace_caps(&[(KEY_SESSION, 1.5, None)]);
        let u = json!({"limits":[{"kind":"session","percent":99.0}]});
        assert_eq!(
            eval(&u, &c),
            Decision::Allow,
            "no resets_at ⇒ no elapsed fraction ⇒ no pace decision"
        );
    }

    #[test]
    fn a_pace_cap_on_a_missing_window_fails_open() {
        let c = pace_caps(&[("weekly_model:ghost", 1.1, None)]);
        assert_eq!(eval(&one_hour_in(99.0), &c), Decision::Allow);
    }

    #[test]
    fn a_scoped_pace_cap_only_burdens_the_model_it_names() {
        let c = pace_caps(&[("weekly_model:fable", 1.5, None)]);
        let u = json!({"limits":[{"kind":"weekly_scoped","percent":90.0,
            "resets_at":"2026-06-23T12:00:00Z",
            "scope":{"model":{"id":null,"display_name":"Fable"}}}]});
        assert!(matches!(eval_for(&u, &c, Some("claude-fable-5")), Decision::Block { .. }));
        assert_eq!(eval_for(&u, &c, Some("claude-opus-4-8")), Decision::Allow);
    }

    #[test]
    fn the_bypass_window_silences_pace_too() {
        // 4h55m into the 5h window: over pace, but the reset is minutes away.
        let c = pace_caps(&[(KEY_SESSION, 1.5, Some(10))]);
        let u = json!({"limits":[{"kind":"session","percent":100.0,
            "resets_at":"2026-06-19T12:05:00Z"}]});
        assert_eq!(eval(&u, &c), Decision::Allow);
    }

    #[test]
    fn a_level_block_outranks_a_pace_block() {
        let mut c = pace_caps(&[(KEY_SESSION, 1.5, None)]);
        c.limits.get_mut(KEY_SESSION).unwrap().cap_pct = Some(50);
        match eval(&one_hour_in(60.0), &c) {
            Decision::Block { retry_after_secs, key, .. } => {
                assert_eq!(key, "session", "the spent budget names the block, not the burn rate");
                assert_eq!(retry_after_secs, 4 * 3600);
            }
            d @ Decision::Allow => panic!("expected a block, got {d:?}"),
        }
    }

    #[test]
    fn a_dollar_window_has_no_pace() {
        let c = pace_caps(&[(KEY_USD_5H, 1.1, None)]);
        assert_eq!(eval(&usd_usage(99.0, "2026-06-19T16:00:00Z", 0.0), &c), Decision::Allow);
    }

    #[test]
    fn a_pace_cap_alone_counts_as_configured_and_round_trips() {
        let sl = SoftLimits::from_json(Some(&json!({
            "session": {"pace_cap": 1.5},
            "weekly_all": {"pace_cap": 0},
        })));
        assert!(!sl.is_unset(), "a pace cap alone must open the evaluation path");
        assert!((sl.limits["session"].pace_cap.unwrap() - 1.5).abs() < 1e-6);
        assert!(!sl.limits.contains_key("weekly_all"), "a non-positive pace cap is dropped");
        assert_eq!(sl, SoftLimits::from_json(Some(&serde_json::to_value(&sl).unwrap())));
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
        match evaluate_soft_limit(&windows, &c, None, now()) {
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
        assert_eq!(evaluate_soft_limit(&[], &c, None, now()), Decision::Allow);
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

    // ---- durable block keys ------------------------------------------------

    #[test]
    fn a_session_scoped_block_survives_every_account_change() {
        let c = usd_caps(&[(KEY_SESSION_USD, 100.0, None)]);
        let windows = vec![usd_window(KEY_SESSION_USD, 5.0, None)];
        let key = format!("{SESSION_SCOPE_PREFIX}{KEY_SESSION_USD}");
        assert!(
            !block_lifted_by(&key, &windows, &c, now()),
            "the session's own budget refused it; no account cap can speak for that"
        );
    }

    #[test]
    fn an_account_block_is_lifted_only_by_its_own_window() {
        let windows = normalize_usage_windows(&legacy(
            90.0,
            "2026-06-19T16:00:00Z",
            95.0,
            "2026-06-26T00:00:00Z",
        ));
        let raised = caps(&[(KEY_SESSION, Some(95), None), (KEY_WEEKLY_ALL, Some(70), None)]);
        assert!(
            block_lifted_by(KEY_SESSION, &windows, &raised, now()),
            "an unrelated window still over cap must not hold the raised one"
        );
        assert!(!block_lifted_by(KEY_WEEKLY_ALL, &windows, &raised, now()));
        assert!(
            block_lifted_by(KEY_SESSION, &windows, &SoftLimits::default(), now()),
            "a removed cap has nothing left to block with"
        );
        assert!(block_lifted_by(
            &format!("{PACE_REASON_PREFIX}{KEY_SESSION}"),
            &windows,
            &SoftLimits::default(),
            now()
        ));
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
