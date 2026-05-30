use serde::{Deserialize, Serialize};

/// A policy rule — either allow or deny a tool call pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyRule {
    /// Tool name to match (e.g. "Bash", "*" for all)
    pub tool: String,
    /// Action: "allow" or "deny"
    pub action: PolicyAction,
    /// Optional pattern to match in the tool input (substring match)
    pub pattern: Option<String>,
    /// Human-readable reason for this rule
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Deny,
}
