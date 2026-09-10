//! Per-account Codex (`~/.codex/config.toml`) settings catalog.
//!
//! The openai-family half of [`super`], sharing its [`Catalog`] /
//! [`SettingKey`] / [`Preset`] types so the endpoint, the validation and the
//! webui editor are family-agnostic. There is no vendored Codex JSON Schema
//! yet, so every entry is `source = "docs"`, hand-maintained against
//! <https://learn.chatgpt.com/docs/config-file/config-reference>.
//!
//! Curation rule for v1 (CCT-986), which the growth path in CCT-709 must keep:
//! **only string-valued keys are exposable.** The daemon delivers overrides as
//! `-c key="value"` with unconditional TOML string quoting, and codex parses
//! that value as TOML — so a quoted boolean (`features.fast_mode="true"`) is a
//! hard app-server startup failure, not a silent no-op. Boolean and numeric keys
//! are catalogued as `system` with a note until typed emission lands.
//!
//! Dotted names (`history.persistence`) are stored literally as top-level JSON
//! keys and map 1:1 onto codex's own `-c` dotted path syntax.

use std::sync::LazyLock;

use super::{Catalog, build_from};

const RAW_CATALOG: &str = include_str!("codex-catalog.toml");

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| build_from("Codex", RAW_CATALOG, None));

/// The process-wide Codex settings catalog singleton.
#[must_use]
pub fn catalog() -> &'static Catalog {
    &CATALOG
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_catalog::{Policy, SettingKey};
    use serde_json::json;

    #[test]
    fn codex_catalog_loads_and_exposes_service_tier() {
        let c = catalog();
        let k = c.key("service_tier").expect("service_tier catalogued");
        assert!(k.account_exposable());
        assert_eq!(k.group.as_deref(), Some("Speed & cost"));
        assert!(k.r#enum.as_deref().is_some_and(|e| e.contains("fast")));
        assert!(c.preset(super::super::QUIET_DEFAULTS_ID).is_some());
    }

    /// Every exposable key must be string-valued: the daemon's `-c key="value"`
    /// emitter cannot round-trip a boolean or a number without bricking the spawn.
    #[test]
    fn exposable_codex_keys_are_string_typed() {
        let bad: Vec<&str> = catalog()
            .exposable_keys()
            .filter(|k| k.r#type.as_deref() != Some("string"))
            .map(|k: &SettingKey| k.name.as_str())
            .collect();
        assert!(bad.is_empty(), "non-string exposable codex keys: {bad:?}");
    }

    #[test]
    fn gateway_critical_codex_keys_are_managed() {
        let c = catalog();
        for name in [
            "model_provider",
            "model_providers",
            "openai_base_url",
            "chatgpt_base_url",
            "approval_policy",
            "sandbox_mode",
            "model",
            "model_reasoning_effort",
        ] {
            let k = c.key(name).unwrap_or_else(|| panic!("{name} catalogued"));
            assert_eq!(k.tag, Policy::Managed, "{name} must be managed");
            assert!(!k.account_exposable());
        }
    }

    #[test]
    fn codex_validation_rejects_managed_and_claude_keys() {
        let c = catalog();
        assert!(c.validate_settings(&json!({"service_tier": "fast"})).ok());
        let r = c.validate_settings(&json!({"model_provider": "cctui"}));
        assert_eq!(r.violations.len(), 1);
        let r = c.validate_settings(&json!({"disableBundledSkills": true}));
        assert!(r.violations[0].reason.contains("Codex"));
    }

}
