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
