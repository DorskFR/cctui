//! Per-account Claude Code settings catalog (CCT-537).
//!
//! This module is the single source of truth for which Claude Code `settings.json`
//! keys and environment variables cctui may expose as per-account defaults, and how
//! dangerous each one is. It is consumed by:
//!
//! - the server's allowlist validation (CCT-538) — reject a pasted settings/env blob
//!   that touches managed-only or unknown keys before persisting it, and
//! - the webui account settings editor (CCT-541) — render the toggle list, grouped by
//!   policy, with types/enums/defaults.
//!
//! ## Two sources, one catalog
//!
//! - `claude-code-settings.schema.json` — the vendored `SchemaStore` JSON Schema
//!   (draft-07, `https://json.schemastore.org/claude-code-settings.json`). It is the
//!   authority for the **types / enums / defaults** of the 84 keys it covers, so those
//!   stay in sync when we re-vendor it. A CI job (`.github/workflows/schema-drift.yml`)
//!   re-fetches it and diffs to flag drift.
//! - `catalog.toml` — the hand-maintained delta the schema lacks: per-key **policy tags**
//!   (`safe`/`care`/`managed`/`system`), the docs-only keys the schema still lags on,
//!   a curated **env-var allowlist** (we do NOT expose all 284 documented vars), and the
//!   named **"Quiet defaults"** preset.
//!
//! Both files are embedded at compile time; there is no runtime fetch or file I/O.
//!
//! The public API is intentionally small and read-only: [`catalog`] returns the parsed
//! singleton, and [`Catalog`] exposes lookups plus [`Catalog::validate_settings`] /
//! [`Catalog::validate_env`] for the server-side allowlist check.

// The public API here is consumed by CCT-538 (server allowlist validation) and
// CCT-541 (webui editor), which land in later waves. Until then it is exercised only
// by this module's tests, so silence dead-code warnings for the additive surface.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};
use serde_json::Value;

const RAW_SCHEMA: &str = include_str!("claude-code-settings.schema.json");
const RAW_CATALOG: &str = include_str!("catalog.toml");

/// Per-key exposure policy. Ordered least → most restrictive for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Policy {
    /// Good per-account toggle candidate; low blast radius.
    Safe,
    /// Exposable but has caveats (cost / security / footgun).
    Care,
    /// Org/admin managed-settings only; must NOT be set per-account.
    Managed,
    /// CLI-written state or session-only; not a user-facing toggle.
    System,
}

impl Policy {
    /// Whether a key/var with this policy may be set from a per-account settings blob.
    /// `safe` and `care` are exposable; `managed` and `system` are not.
    #[must_use]
    pub const fn account_exposable(self) -> bool {
        matches!(self, Self::Safe | Self::Care)
    }

    /// Lowercase tag string (`"safe"`, `"care"`, `"managed"`, `"system"`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Safe => "safe",
            Self::Care => "care",
            Self::Managed => "managed",
            Self::System => "system",
        }
    }
}

/// Where a key's definition comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Present in the vendored JSON schema (types/enums/defaults come from it).
    Schema,
    /// Docs-only key the schema still lags on (metadata hand-maintained here).
    Docs,
}

/// A single `settings.json` top-level key, enriched from the schema where possible.
#[derive(Debug, Clone, Serialize)]
pub struct SettingKey {
    /// Top-level key name (e.g. `"model"`, `"disableBundledSkills"`).
    pub name: String,
    /// Exposure policy tag.
    pub tag: Policy,
    /// Schema vs docs origin.
    pub source: Source,
    /// JSON type(s), e.g. `"boolean"`, `"string"`, `"array"` (best-effort).
    pub r#type: Option<String>,
    /// Allowed enum values, comma-joined, when the key is an enum.
    pub r#enum: Option<String>,
    /// Documented default, as a display string.
    pub default: Option<String>,
    /// Human-readable notes (from the schema description or the hand catalog).
    pub notes: Option<String>,
}

impl SettingKey {
    /// Convenience: may this key be set from a per-account settings blob?
    #[must_use]
    pub const fn account_exposable(&self) -> bool {
        self.tag.account_exposable()
    }
}

/// A curated environment variable exposed as an account default.
#[derive(Debug, Clone, Serialize)]
pub struct EnvVar {
    /// Variable name (e.g. `"ANTHROPIC_MODEL"`).
    pub name: String,
    /// Grouping for the editor UI (`model`/`context`/`thinking`/`tokens`/`skills`/
    /// `timeouts`/`telemetry`).
    pub group: String,
    /// Exposure policy tag.
    pub tag: Policy,
    /// Human-readable notes.
    pub notes: Option<String>,
}

/// A named bundle of settings + env applied together (e.g. "Quiet defaults").
#[derive(Debug, Clone, Serialize)]
pub struct Preset {
    /// Stable id (e.g. `"quiet-defaults"`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// What it does / caveats.
    pub description: String,
    /// `settings.json` fragment this preset writes.
    pub settings: BTreeMap<String, Value>,
    /// Env fragment this preset writes.
    pub env: BTreeMap<String, String>,
}

/// The `"quiet-defaults"` preset id, referenced by the webui and server.
pub const QUIET_DEFAULTS_ID: &str = "quiet-defaults";

/// A single allowlist violation from [`Catalog::validate_settings`] /
/// [`Catalog::validate_env`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Violation {
    /// Offending key / env-var name.
    pub key: String,
    /// Why it was rejected.
    pub reason: String,
}

/// Outcome of validating a settings or env blob against the catalog allowlist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ValidationReport {
    /// Keys that are not exposable per-account (unknown, managed, or system).
    pub violations: Vec<Violation>,
}

impl ValidationReport {
    /// Whether the blob is safe to persist (no violations).
    #[must_use]
    pub const fn ok(&self) -> bool {
        self.violations.is_empty()
    }
}

/// The parsed, enriched settings catalog. Access the singleton via [`catalog`].
#[derive(Debug)]
pub struct Catalog {
    keys: Vec<SettingKey>,
    keys_by_name: BTreeMap<String, usize>,
    env: Vec<EnvVar>,
    env_by_name: BTreeMap<String, usize>,
    presets: Vec<Preset>,
    /// URL the schema was vendored from (for the drift CI job / provenance).
    pub schema_url: String,
}

impl Catalog {
    /// All `settings.json` keys, schema + docs delta, in catalog order.
    #[must_use]
    pub fn keys(&self) -> &[SettingKey] {
        &self.keys
    }

    /// Look up a single key by exact name.
    #[must_use]
    pub fn key(&self, name: &str) -> Option<&SettingKey> {
        self.keys_by_name.get(name).map(|&i| &self.keys[i])
    }

    /// The curated env-var allowlist (NOT all 284 documented vars).
    #[must_use]
    pub fn env_allowlist(&self) -> &[EnvVar] {
        &self.env
    }

    /// Look up a single curated env var by exact name.
    #[must_use]
    pub fn env(&self, name: &str) -> Option<&EnvVar> {
        self.env_by_name.get(name).map(|&i| &self.env[i])
    }

    /// All named presets.
    #[must_use]
    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// Look up a preset by id.
    #[must_use]
    pub fn preset(&self, id: &str) -> Option<&Preset> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The "Quiet defaults" preset. Present in the catalog by construction; panics at
    /// load time (in [`build`]) if it is ever removed, so this cannot return `None`.
    #[must_use]
    pub fn quiet_defaults(&self) -> &Preset {
        self.preset(QUIET_DEFAULTS_ID).expect("quiet-defaults preset present (checked at load)")
    }

    /// Validate a `settings.json` object against the per-account allowlist policy.
    ///
    /// Each top-level key must be known to the catalog AND tagged `safe`/`care`.
    /// Unknown keys, `managed`, and `system` keys are violations. `value` must be a
    /// JSON object; a non-object is a single violation.
    ///
    /// Note: this checks TOP-LEVEL keys only. Nested footguns (e.g.
    /// `permissions.defaultMode: bypassPermissions`) are out of scope here and are the
    /// injection layer's concern (CCT-538/CCT-539).
    #[must_use]
    pub fn validate_settings(&self, value: &Value) -> ValidationReport {
        let mut violations = Vec::new();
        let Some(obj) = value.as_object() else {
            violations.push(Violation {
                key: "$".to_string(),
                reason: "settings must be a JSON object".to_string(),
            });
            return ValidationReport { violations };
        };
        for name in obj.keys() {
            match self.key(name) {
                None => violations.push(Violation {
                    key: name.clone(),
                    reason: "unknown settings key (not in the Claude Code catalog)".to_string(),
                }),
                Some(k) if !k.account_exposable() => violations.push(Violation {
                    key: name.clone(),
                    reason: format!(
                        "key is tagged `{}` and cannot be set per-account",
                        k.tag.as_str()
                    ),
                }),
                Some(_) => {}
            }
        }
        ValidationReport { violations }
    }

    /// Validate an env map against the curated allowlist. Every name must be in the
    /// allowlist and tagged `safe`/`care` (the allowlist only contains such vars, but
    /// the tag is checked defensively).
    #[must_use]
    pub fn validate_env(&self, env: &BTreeMap<String, String>) -> ValidationReport {
        let mut violations = Vec::new();
        for name in env.keys() {
            match self.env(name) {
                None => violations.push(Violation {
                    key: name.clone(),
                    reason: "env var not in the curated per-account allowlist".to_string(),
                }),
                Some(v) if !v.tag.account_exposable() => violations.push(Violation {
                    key: name.clone(),
                    reason: format!("env var is tagged `{}`", v.tag.as_str()),
                }),
                Some(_) => {}
            }
        }
        ValidationReport { violations }
    }
}

// ---- loading / parsing ----------------------------------------------------

/// Raw TOML shape of `catalog.toml`.
#[derive(Debug, Deserialize)]
struct RawCatalog {
    schema_url: String,
    #[serde(default)]
    keys: Vec<RawKey>,
    #[serde(default)]
    env: Vec<RawEnv>,
    #[serde(default)]
    presets: Vec<RawPreset>,
}

#[derive(Debug, Deserialize)]
struct RawKey {
    name: String,
    tag: Policy,
    source: Source,
    r#type: Option<String>,
    r#enum: Option<String>,
    default: Option<String>,
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawEnv {
    name: String,
    group: String,
    tag: Policy,
    notes: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPreset {
    id: String,
    name: String,
    description: String,
    #[serde(default)]
    settings: BTreeMap<String, Value>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

/// Best-effort extraction of `type` / `enum` / `default` / `description` for one key
/// from the vendored schema's `properties` map.
fn enrich_from_schema(
    props: Option<&Value>,
    name: &str,
) -> (Option<String>, Option<String>, Option<String>, Option<String>) {
    let Some(p) = props.and_then(|p| p.get(name)) else {
        return (None, None, None, None);
    };
    let type_ = match p.get("type") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Array(a)) => {
            Some(a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(" | "))
        }
        _ => None,
    };
    let enum_ = p.get("enum").and_then(Value::as_array).map(|a| {
        a.iter()
            .map(|v| match v {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect::<Vec<_>>()
            .join(", ")
    });
    let default = p.get("default").map(|d| match d {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    });
    let desc = p.get("description").and_then(Value::as_str).map(str::to_string);
    (type_, enum_, default, desc)
}

/// Parse both embedded artifacts into a [`Catalog`]. Panics on malformed embedded data
/// (a build-time invariant — the files are checked into the repo and covered by tests).
fn build() -> Catalog {
    let raw: RawCatalog = toml::from_str(RAW_CATALOG).expect("catalog.toml parses");
    let schema: Value = serde_json::from_str(RAW_SCHEMA).expect("vendored schema parses");
    let props = schema.get("properties");

    let mut keys = Vec::with_capacity(raw.keys.len());
    let mut keys_by_name = BTreeMap::new();
    for (i, k) in raw.keys.into_iter().enumerate() {
        let (s_type, s_enum, s_default, s_desc) = if matches!(k.source, Source::Schema) {
            enrich_from_schema(props, &k.name)
        } else {
            (None, None, None, None)
        };
        keys_by_name.insert(k.name.clone(), i);
        keys.push(SettingKey {
            name: k.name,
            tag: k.tag,
            source: k.source,
            // Hand catalog wins where present (docs delta); otherwise use the schema.
            r#type: k.r#type.or(s_type),
            r#enum: k.r#enum.or(s_enum),
            default: k.default.or(s_default),
            notes: k.notes.or(s_desc),
        });
    }

    let mut env = Vec::with_capacity(raw.env.len());
    let mut env_by_name = BTreeMap::new();
    for (i, e) in raw.env.into_iter().enumerate() {
        env_by_name.insert(e.name.clone(), i);
        env.push(EnvVar { name: e.name, group: e.group, tag: e.tag, notes: e.notes });
    }

    let presets: Vec<Preset> = raw
        .presets
        .into_iter()
        .map(|p| Preset {
            id: p.id,
            name: p.name,
            description: p.description,
            settings: p.settings,
            env: p.env,
        })
        .collect();

    assert!(
        presets.iter().any(|p| p.id == QUIET_DEFAULTS_ID),
        "catalog.toml must define the `{QUIET_DEFAULTS_ID}` preset"
    );

    Catalog { keys, keys_by_name, env, env_by_name, presets, schema_url: raw.schema_url }
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(build);

/// The process-wide settings catalog singleton.
#[must_use]
pub fn catalog() -> &'static Catalog {
    &CATALOG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_loads_and_indexes() {
        let c = catalog();
        assert!(c.keys().len() >= 100, "expected the full key catalog");
        assert!(!c.env_allowlist().is_empty());
        // Index round-trips.
        for k in c.keys() {
            assert!(c.key(&k.name).is_some(), "{} not indexed", k.name);
        }
        for e in c.env_allowlist() {
            assert!(c.env(&e.name).is_some());
        }
    }

    /// Drift guard: every property in the vendored schema must have a catalog entry
    /// tagged `source = schema`. If `SchemaStore` adds a key, re-vendor + tag it.
    #[test]
    fn every_schema_property_is_catalogued() {
        let c = catalog();
        let schema: Value = serde_json::from_str(RAW_SCHEMA).unwrap();
        let props =
            schema.get("properties").and_then(Value::as_object).expect("schema has properties");
        let missing: Vec<&String> = props
            .keys()
            .filter(|name| !matches!(c.key(name), Some(k) if matches!(k.source, Source::Schema)))
            .collect();
        assert!(
            missing.is_empty(),
            "schema properties missing from catalog.toml (source=schema): {missing:?}"
        );
    }

    #[test]
    fn schema_keys_enriched_from_schema() {
        // `model` is a schema key; it should have carried a type from the schema.
        let k = catalog().key("model").expect("model key present");
        assert_eq!(k.source, Source::Schema);
        assert!(k.r#type.is_some(), "model type should come from the schema");
        // An enum key carries its allowed values.
        let e = catalog().key("effortLevel").expect("effortLevel present");
        assert!(e.r#enum.as_deref().is_some_and(|s| s.contains("high")));
    }

    #[test]
    fn docs_only_keys_present() {
        for name in ["disableBundledSkills", "disableWorkflows", "disableArtifact", "fallbackModel"]
        {
            let k = catalog().key(name).unwrap_or_else(|| panic!("{name} missing"));
            assert_eq!(k.source, Source::Docs, "{name} should be a docs-delta key");
        }
    }

    #[test]
    fn validate_settings_allows_safe_rejects_managed_and_unknown() {
        let c = catalog();
        let ok = serde_json::json!({ "model": "x", "disableBundledSkills": true });
        assert!(c.validate_settings(&ok).ok(), "safe keys should pass");

        let bad = serde_json::json!({
            "forceLoginMethod": "console", // managed
            "totallyNotAKey": 1            // unknown
        });
        let report = c.validate_settings(&bad);
        assert!(!report.ok());
        let keys: Vec<&str> = report.violations.iter().map(|v| v.key.as_str()).collect();
        assert!(keys.contains(&"forceLoginMethod"));
        assert!(keys.contains(&"totallyNotAKey"));

        // Non-object payload is a single violation.
        assert!(!c.validate_settings(&serde_json::json!([1, 2, 3])).ok());
    }

    #[test]
    fn validate_env_enforces_allowlist() {
        let c = catalog();
        let mut ok = BTreeMap::new();
        ok.insert("ANTHROPIC_MODEL".to_string(), "claude-x".to_string());
        assert!(c.validate_env(&ok).ok());

        let mut bad = BTreeMap::new();
        bad.insert("ANTHROPIC_API_KEY".to_string(), "sk-secret".to_string());
        assert!(!c.validate_env(&bad).ok());
    }

    #[test]
    fn quiet_defaults_preset_is_complete() {
        let p = catalog().quiet_defaults();
        assert_eq!(p.id, QUIET_DEFAULTS_ID);
        assert_eq!(p.settings.get("disableBundledSkills"), Some(&Value::Bool(true)));
        assert_eq!(p.settings.get("remoteControlAtStartup"), Some(&Value::Bool(false)));
        assert_eq!(
            p.env.get("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC").map(String::as_str),
            Some("1")
        );
        // The preset's own settings must all be known catalog keys (though some are
        // MANAGED — the preset is applied by cctui, not validated as user input).
        let c = catalog();
        for name in p.settings.keys() {
            assert!(c.key(name).is_some(), "preset key {name} not in catalog");
        }
    }
}
