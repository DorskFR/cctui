//! The embedded `settings.json` / env catalog, as served to the account editor.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(feature = "ts")]
use ts_rs::TS;

/// Per-key exposure policy. Ordered least → most restrictive for display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
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

/// Where a key's definition comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Present in the vendored JSON schema (types/enums/defaults come from it).
    Schema,
    /// Docs-only key the schema still lags on (metadata hand-maintained here).
    Docs,
}

/// A single `settings.json` top-level key, enriched from the schema where possible.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
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
    /// Editor grouping for the account-settings toggle list. Set in
    /// catalog.toml on the curated boolean keys only; a key with a group gets a
    /// tri-state toggle in the webui, everything else is raw-JSON-only.
    pub group: Option<String>,
    /// Human-readable toggle label (paired with `group`).
    pub label: Option<String>,
}

/// Control shape for a curated env var in the account-settings editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum EnvKind {
    /// Present as `"1"` / absent (a boolean switch).
    #[default]
    Flag,
    /// A numeric literal stored as a string.
    Number,
    /// A free-form string literal.
    String,
    /// One of a fixed set of values (`values`).
    Enum,
}

/// A curated environment variable exposed as an account default.
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct EnvVar {
    /// Variable name (e.g. `"ANTHROPIC_MODEL"`).
    pub name: String,
    /// Grouping for the editor UI (`model`/`context`/`thinking`/`tokens`/`skills`/
    /// `timeouts`/`telemetry`).
    pub group: String,
    /// Exposure policy tag.
    pub tag: Policy,
    /// Control shape rendered by the editor.
    pub kind: EnvKind,
    /// Allowed values for an `enum`-kind var (e.g. effort levels).
    pub values: Option<Vec<String>>,
    /// settings.json key this env var aliases, when one exists — the
    /// editor merges the two into ONE row that reads/writes the settings key.
    pub settings_equiv: Option<String>,
    /// Another env var this is an exact alias of (`DO_NOT_TRACK` == `DISABLE_TELEMETRY`).
    /// The editor folds aliases into the primary's row so only one renders.
    pub env_alias_of: Option<String>,
    /// Human-readable row label (like curated settings keys have).
    pub label: Option<String>,
    /// Human-readable notes.
    pub notes: Option<String>,
}

/// A named bundle of settings + env applied together (e.g. "Quiet defaults").
#[derive(Debug, Clone, Serialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
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

impl SettingKey {
    /// Convenience: may this key be set from a per-account settings blob?
    #[must_use]
    pub const fn account_exposable(&self) -> bool {
        self.tag.account_exposable()
    }
}
