//! Per-account Codex (`~/.codex/config.toml`) settings catalog.
//!
//! The openai-family half of [`super`], sharing its [`Catalog`] /
//! [`SettingKey`] / [`Preset`] types so the endpoint, the validation and the
//! webui editor are family-agnostic. `codex-config.schema.json` is codex's own
//! `config.schema.json` vendored at the `CODEX_MIN_VERSION` release; keys it
//! covers are `source = "schema"`, the rest are hand-maintained against
//! <https://learn.chatgpt.com/docs/config-file/config-reference>.
//!
//! Curation rule: a key is exposable only if
//! [`cctui_proto::codex_config::CURATED`] can render it into a session, and the
//! two lists are held to exact agreement by a test here. Anything cctui sets
//! per session itself (gateway routing, permission posture, model/effort) stays
//! `managed`, so an account value can never clobber it.
//!
//! Dotted names (`history.persistence`) are stored literally as top-level JSON
//! keys and map 1:1 onto codex's own `-c` dotted path syntax.

use std::sync::LazyLock;

use super::{Catalog, build_from};

const RAW_CATALOG: &str = include_str!("codex-catalog.toml");
const RAW_SCHEMA: &str = include_str!("codex-config.schema.json");

pub use cctui_server_core::execution_settings::*;

static CATALOG: LazyLock<Catalog> = LazyLock::new(|| build_from(RAW_CATALOG, Some(RAW_SCHEMA)));

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
    use std::collections::BTreeSet;

    /// The tier is gated on the adapter, not the provider family: qwen and
    /// goose are OpenAI-family too and get none.
    #[test]
    fn only_codex_and_its_variants_take_a_service_tier() {
        assert!(adapter_takes_service_tier("codex"));
        assert!(adapter_takes_service_tier("codex-app-server"));
        for other in ["qwen", "goose", "claude-code", "opencode", "gemini", ""] {
            assert!(!adapter_takes_service_tier(other), "{other:?}");
        }
    }

    #[test]
    fn codex_catalog_loads_and_exposes_service_tier() {
        let c = catalog();
        let k = c.key(SERVICE_TIER_KEY).expect("service_tier catalogued");
        assert!(k.account_exposable());
        assert_eq!(k.group.as_deref(), Some("Speed & cost"));
        assert!(k.r#enum.as_deref().is_some_and(|e| e.contains("fast")));
        assert!(c.preset(super::super::QUIET_DEFAULTS_ID).is_some());
    }

    /// The catalog says what an account MAY set; `codex_config::CURATED` says
    /// what actually gets rendered into a session. A key in one and not the
    /// other is either an unsettable toggle or an unvetted injection, so the two
    /// must agree exactly — `service_tier` excepted, which is stored per account
    /// but supplied per thread rather than rendered.
    #[test]
    fn exposable_codex_keys_match_the_renderer() {
        let exposable: BTreeSet<&str> = catalog()
            .exposable_keys()
            .map(|k: &SettingKey| k.name.as_str())
            .filter(|n| *n != SERVICE_TIER_KEY)
            .collect();
        let rendered: BTreeSet<&str> =
            cctui_proto::codex_config::CURATED.iter().map(|c| c.name).collect();
        assert_eq!(exposable, rendered);
    }

    /// A catalogued type that disagrees with the renderer's TOML type emits a
    /// literal codex will reject, so the declared types must line up too.
    #[test]
    fn catalogued_types_match_the_rendered_toml_types() {
        use cctui_proto::codex_config::TomlType;
        for c in cctui_proto::codex_config::CURATED {
            let k = catalog().key(c.name).unwrap_or_else(|| panic!("{} catalogued", c.name));
            let want = match c.ty {
                TomlType::Str => "string",
                TomlType::Bool => "boolean",
                TomlType::Int => "number",
            };
            assert_eq!(k.r#type.as_deref(), Some(want), "{} type mismatch", c.name);
        }
    }

    /// The preset is applied by writing it straight into `settings_json`, so
    /// every value in it must survive validation and actually render.
    #[test]
    fn quiet_defaults_preset_validates_and_renders() {
        let c = catalog();
        let p = c.preset(super::super::QUIET_DEFAULTS_ID).expect("preset present");
        let blob = serde_json::Value::Object(
            p.settings.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        );
        assert!(c.validate_settings(&blob).ok(), "preset must pass its own validation");
        assert_eq!(
            cctui_proto::codex_config::render_lines(&blob).len(),
            p.settings.len(),
            "every preset key must render"
        );
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
    fn codex_validation_rejects_managed_flags_unknown_keys() {
        let c = catalog();
        assert!(c.validate_settings(&json!({"service_tier": "fast"})).ok());
        let r = c.validate_settings(&json!({"model_provider": "cctui"}));
        assert_eq!(r.violations.len(), 1);
        let r = c.validate_settings(&json!({"disableBundledSkills": true}));
        assert!(r.ok());
        assert_eq!(r.out_of_schema, ["disableBundledSkills"]);
    }

    #[test]
    fn service_tier_resolution_defaults_to_standard_tier() {
        assert_eq!(resolve_service_tier(None, None), SERVICE_TIER_DEFAULT);
        assert_eq!(resolve_service_tier(Some("fast"), None), SERVICE_TIER_FAST);
        assert_eq!(resolve_service_tier(Some(" FAST "), None), SERVICE_TIER_FAST);
        assert_eq!(resolve_service_tier(Some("priority"), None), SERVICE_TIER_DEFAULT);
        assert_eq!(resolve_service_tier(Some(""), None), SERVICE_TIER_DEFAULT);
    }

    #[test]
    fn account_default_applies_only_without_a_session_choice() {
        let acct = json!({"service_tier": "fast"});
        assert_eq!(resolve_service_tier(None, Some(&acct)), SERVICE_TIER_FAST);
        assert_eq!(resolve_service_tier(Some("default"), Some(&acct)), SERVICE_TIER_DEFAULT);
        assert_eq!(
            resolve_service_tier(None, Some(&json!({"service_tier": "nope"}))),
            SERVICE_TIER_DEFAULT
        );
        assert_eq!(resolve_service_tier(None, Some(&json!({}))), SERVICE_TIER_DEFAULT);
    }

    #[test]
    fn overlay_pins_a_tier_and_preserves_the_account_default() {
        assert_eq!(overlay_service_tier(None), Some(json!({"service_tier": "default"})));
        assert_eq!(
            overlay_service_tier(Some(json!({"history.persistence": "none"}))),
            Some(json!({"history.persistence": "none", "service_tier": "default"}))
        );
        assert_eq!(
            overlay_service_tier(Some(json!({"service_tier": "fast"}))),
            Some(json!({"service_tier": "fast"}))
        );
        assert_eq!(
            overlay_service_tier(Some(json!({"service_tier": "priority"}))),
            Some(json!({"service_tier": "default"}))
        );
    }
}
