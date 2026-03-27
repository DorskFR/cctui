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

/// Evaluate a set of rules against a tool call.
/// Rules are evaluated in order; first match wins. Default is allow.
pub fn evaluate(rules: &[PolicyRule], tool: &str, input: &serde_json::Value) -> PolicyDecision {
    let input_str = input.to_string();

    for rule in rules {
        let tool_matches = rule.tool == "*" || rule.tool == tool;
        if !tool_matches {
            continue;
        }

        let pattern_matches = rule.pattern.as_deref().is_none_or(|p| input_str.contains(p));

        if pattern_matches {
            return match rule.action {
                PolicyAction::Allow => PolicyDecision::Allow,
                PolicyAction::Deny => PolicyDecision::Deny {
                    reason: rule
                        .reason
                        .clone()
                        .unwrap_or_else(|| format!("Denied by policy: {tool}")),
                },
            };
        }
    }

    PolicyDecision::Allow // default: allow all
}

#[derive(Debug)]
pub enum PolicyDecision {
    Allow,
    Deny { reason: String },
}
