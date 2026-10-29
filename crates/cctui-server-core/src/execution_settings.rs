//! Execution settings independent of workspace catalog editors.

/// The `service_tier` value pinning a session to the standard (non-Fast) tier.
///
/// Codex's own default is `priority` for every gpt-5.x model, so an unset tier is
/// the expensive one — cctui pins this explicitly rather than inheriting.
pub const SERVICE_TIER_DEFAULT: &str = "default";

/// The `service_tier` value for Fast mode (1.5x speed, increased usage — same
/// model, same quality). Codex maps it to the request value `priority`.
pub const SERVICE_TIER_FAST: &str = "fast";

/// The `settings_json` key carrying the account-level tier default.
pub const SERVICE_TIER_KEY: &str = "service_tier";

/// Normalise a requested service tier: `default`/`fast` pass through, blank and
/// anything unrecognised become [`None`] so the next level decides.
#[must_use]
pub fn normalize_service_tier(raw: Option<&str>) -> Option<String> {
    let v = raw?.trim().to_ascii_lowercase();
    matches!(v.as_str(), SERVICE_TIER_DEFAULT | SERVICE_TIER_FAST).then_some(v)
}

/// The account-level tier default from a provider's `settings_json` blob.
#[must_use]
pub fn service_tier_from_settings(settings: Option<&serde_json::Value>) -> Option<String> {
    normalize_service_tier(settings?.get(SERVICE_TIER_KEY)?.as_str())
}

/// Resolve the service tier for a Codex session.
///
/// An explicit per-session choice wins over
/// the account default, and with neither the session is pinned to the standard
/// tier rather than inheriting codex's `priority` default.
#[must_use]
pub fn resolve_service_tier(
    requested: Option<&str>,
    account_settings: Option<&serde_json::Value>,
) -> String {
    normalize_service_tier(requested)
        .or_else(|| service_tier_from_settings(account_settings))
        .unwrap_or_else(|| SERVICE_TIER_DEFAULT.to_owned())
}

/// Only codex reads a `service_tier`. The other OpenAI-family harnesses are
/// ACP agents on the chat wire, which would carry a key they cannot use.
#[must_use]
pub fn adapter_takes_service_tier(adapter_id: &str) -> bool {
    cctui_proto::adapter::harness_for_adapter(adapter_id)
        .is_some_and(|h| h.id == cctui_proto::harness::HARNESS_CODEX)
}

/// Pin an explicit service tier into daemon launch settings.
///
/// The concrete `service_tier` is served to the daemon on
/// every worker (re)launch, so a codex session never falls back to codex's own
/// `priority` default. An account default already in the blob is preserved.
#[must_use]
pub fn overlay_service_tier(settings: Option<serde_json::Value>) -> Option<serde_json::Value> {
    let tier = resolve_service_tier(None, settings.as_ref());
    let mut blob = settings.unwrap_or_else(|| serde_json::Value::Object(serde_json::Map::new()));
    let obj = blob.as_object_mut()?;
    obj.insert(SERVICE_TIER_KEY.to_owned(), serde_json::Value::String(tier));
    Some(blob)
}

use serde_json::{Value, json};

/// Map a user-facing harness mode (`bg`/`sdk`/`oneshot`, or unknown) to the
///
/// claude-code adapter's internal `config["mode"]` token. Today `bg`
/// is served by the existing `claude-daemon` path; `sdk`/`oneshot` pass through
/// as-is. Any unknown/missing value defaults to `bg`.
#[must_use]
pub fn harness_mode_to_adapter_token(harness_mode: Option<&str>) -> String {
    let mode = harness_mode.filter(|m| ["bg", "sdk", "oneshot"].contains(m)).unwrap_or("bg");
    match mode {
        "bg" => "claude-daemon".to_owned(),
        other => other.to_owned(),
    }
}

const MAX_SCRUB_PATTERNS: usize = 100;
const MAX_SCRUB_NAME_CHARS: usize = 60;
const MAX_SCRUB_REGEX_CHARS: usize = 400;

/// Normalize `secretScrubPatterns` into a clamped array of
///
/// `{ name, regex, enabled }`. Each entry is trimmed, length-capped, and its
/// `regex` is **rejected unless it compiles** (returned in `Err` so `PUT` can
/// 400). Duplicates by regex are dropped and the count is capped. Returns an
/// empty vec when the key is absent or reduces to nothing.
pub fn normalize_scrub_patterns(raw: Option<&Value>) -> Result<Vec<Value>, String> {
    let Some(arr) = raw.and_then(Value::as_array) else { return Ok(Vec::new()) };
    let mut out: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for entry in arr {
        let name: String = entry
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .chars()
            .take(MAX_SCRUB_NAME_CHARS)
            .collect();
        let regex: String = entry
            .get("regex")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .chars()
            .take(MAX_SCRUB_REGEX_CHARS)
            .collect();
        if regex.is_empty() {
            continue;
        }
        if let Err(e) = cctui_crypto::redact::validate_regex(&regex) {
            return Err(format!("invalid scrub regex {regex:?}: {e}"));
        }
        if seen.iter().any(|r| r == &regex) {
            continue;
        }
        let enabled = entry.get("enabled").and_then(Value::as_bool).unwrap_or(true);
        seen.push(regex.clone());
        let name = if name.is_empty() { "custom".to_owned() } else { name };
        out.push(json!({ "name": name, "regex": regex, "enabled": enabled }));
        if out.len() >= MAX_SCRUB_PATTERNS {
            break;
        }
    }
    Ok(out)
}

/// Clamp `data.secretScrubEnabled` / `data.secretScrubPatterns` in place on
///
/// write, returning `Err(msg)` when a user regex does not compile so
/// `put_settings` can reject the PUT with a 400.
pub fn clamp_secret_scrub(data: &mut Value) -> Result<(), String> {
    let Some(obj) = data.as_object_mut() else { return Ok(()) };
    if obj.contains_key("secretScrubEnabled") {
        let on = obj.get("secretScrubEnabled").and_then(Value::as_bool).unwrap_or(true);
        obj.insert("secretScrubEnabled".to_owned(), Value::Bool(on));
    }
    if obj.contains_key("secretScrubPatterns") {
        let patterns = normalize_scrub_patterns(obj.get("secretScrubPatterns"))?;
        if patterns.is_empty() {
            obj.remove("secretScrubPatterns");
        } else {
            obj.insert("secretScrubPatterns".to_owned(), Value::Array(patterns));
        }
    }
    Ok(())
}

/// The effective [`SecretScrubConfig`] for the daemon Reconcile: the
/// enable flag plus the enabled, validated user patterns from a settings blob.
#[must_use]
pub fn secret_scrub_of(data: &Value) -> cctui_proto::ws::SecretScrubConfig {
    let enabled = data.get("secretScrubEnabled").and_then(Value::as_bool).unwrap_or(true);
    let patterns = normalize_scrub_patterns(data.get("secretScrubPatterns"))
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.get("enabled").and_then(Value::as_bool).unwrap_or(true))
        .filter_map(|p| {
            Some(cctui_proto::ws::ScrubPattern {
                name: p.get("name")?.as_str()?.to_owned(),
                regex: p.get("regex")?.as_str()?.to_owned(),
            })
        })
        .collect();
    cctui_proto::ws::SecretScrubConfig { enabled, patterns }
}
