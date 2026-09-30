//! Account-provider metadata: display names, families, credential kind.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// An account may hold at most one provider per family; mirrors the server's
/// generated `family` column.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ProviderFamily {
    #[default]
    Anthropic,
    Openai,
    Fireworks,
}

/// One selectable provider kind, in the order the pickers list them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ProviderInfo {
    pub id: String,
    /// Short display name (`anthropic` → `Claude`).
    pub label: String,
    /// Longer form the provider pickers list.
    pub picker_label: String,
    pub family: ProviderFamily,
    /// Whether the credential is a static key the gateway forwards (no OAuth).
    pub static_credential: bool,
}

/// A quota probe the server's registry serves, for the `usage_probe` picker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct UsageProbeInfo {
    pub id: String,
    pub label: String,
}

#[must_use]
pub fn provider_family(id: &str) -> ProviderFamily {
    if id == "fireworks" {
        ProviderFamily::Fireworks
    } else if id.starts_with("openai") {
        ProviderFamily::Openai
    } else {
        ProviderFamily::Anthropic
    }
}

/// The `*-compatible` endpoints and Fireworks authenticate with a static key.
#[must_use]
pub fn is_static_credential(id: &str) -> bool {
    id.ends_with("-compatible") || id == "fireworks"
}

#[must_use]
pub fn provider_label(id: &str) -> String {
    match id {
        "anthropic" => "Claude",
        "openai" => "Codex",
        "anthropic-compatible" => "Anthropic-compatible",
        "openai-compatible" => "OpenAI-compatible",
        "fireworks" => "Fireworks",
        other => other,
    }
    .to_owned()
}

fn info(id: &str, picker_label: &str) -> ProviderInfo {
    ProviderInfo {
        id: id.to_owned(),
        label: provider_label(id),
        picker_label: picker_label.to_owned(),
        family: provider_family(id),
        static_credential: is_static_credential(id),
    }
}

#[must_use]
pub fn provider_kinds() -> Vec<ProviderInfo> {
    vec![
        info("anthropic", "Claude (anthropic)"),
        info("openai", "Codex (openai)"),
        info("anthropic-compatible", "Anthropic-compatible endpoint"),
        info("openai-compatible", "OpenAI-compatible endpoint"),
        info("fireworks", "Fireworks"),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_ids_get_a_display_name_and_an_unknown_one_stays_itself() {
        assert_eq!(provider_label("anthropic"), "Claude");
        assert_eq!(provider_label("openai"), "Codex");
        assert_eq!(provider_label("anthropic-compatible"), "Anthropic-compatible");
        assert_eq!(provider_label("openai-compatible"), "OpenAI-compatible");
        assert_eq!(provider_label("fireworks"), "Fireworks");
        assert_eq!(provider_label("something-else"), "something-else");
    }

    #[test]
    fn families_match_the_generated_column() {
        assert_eq!(provider_family("anthropic"), ProviderFamily::Anthropic);
        assert_eq!(provider_family("anthropic-compatible"), ProviderFamily::Anthropic);
        assert_eq!(provider_family("openai"), ProviderFamily::Openai);
        assert_eq!(provider_family("openai-compatible"), ProviderFamily::Openai);
        assert_eq!(provider_family("fireworks"), ProviderFamily::Fireworks);
        assert_eq!(provider_family("mystery"), ProviderFamily::Anthropic);
    }

    #[test]
    fn only_key_based_providers_are_static_credentials() {
        assert!(is_static_credential("anthropic-compatible"));
        assert!(is_static_credential("openai-compatible"));
        assert!(is_static_credential("fireworks"));
        assert!(!is_static_credential("anthropic"));
        assert!(!is_static_credential("openai"));
    }

    #[test]
    fn the_picker_order_is_stable() {
        assert_eq!(
            provider_kinds().iter().map(|p| p.id.as_str()).collect::<Vec<_>>(),
            vec!["anthropic", "openai", "anthropic-compatible", "openai-compatible", "fireworks"]
        );
    }
}
