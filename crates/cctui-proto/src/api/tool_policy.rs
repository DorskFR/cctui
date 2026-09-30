//! `/api/v1/accounts/{id}/tool-policy` — the stored workflow-guard policy.

/// The stored, editable form of an account's policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ToolPolicy {
    /// Case-insensitive literals.
    #[serde(default)]
    pub terms: Vec<String>,
    /// Case-insensitive regular expressions.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// GitHub owners whose `owner/repo#n` references and PR/issue URLs are
    /// blocked.
    #[serde(default)]
    pub protected_owners: Vec<String>,
    /// Sessions whose cwd is under one of these roots are not scanned.
    #[serde(default)]
    pub exempt_roots: Vec<String>,
}
