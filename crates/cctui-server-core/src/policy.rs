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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_action_serializes_snake_case() {
        assert_eq!(serde_json::to_string(&PolicyAction::Allow).unwrap(), "\"allow\"");
        assert_eq!(serde_json::to_string(&PolicyAction::Deny).unwrap(), "\"deny\"");
        assert_eq!(serde_json::from_str::<PolicyAction>("\"deny\"").unwrap(), PolicyAction::Deny);
        assert!(serde_json::from_str::<PolicyAction>("\"nope\"").is_err());
    }

    #[test]
    fn policy_rule_round_trips_with_optional_fields() {
        let rule = PolicyRule {
            tool: "Bash".into(),
            action: PolicyAction::Deny,
            pattern: Some("rm -rf".into()),
            reason: None,
        };
        let json = serde_json::to_value(&rule).unwrap();
        assert_eq!(json["tool"], "Bash");
        assert_eq!(json["action"], "deny");
        assert_eq!(json["pattern"], "rm -rf");
        let back: PolicyRule = serde_json::from_value(json).unwrap();
        assert_eq!(back.tool, "Bash");
        assert_eq!(back.action, PolicyAction::Deny);
    }
}
