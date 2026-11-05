//! Harness-neutral hook policy: rules cctui owns, converted per harness into
//! its native hook configuration at launch.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

pub const MAX_COMMAND_BYTES: usize = 4096;
pub const MAX_RULES: usize = 64;
pub const MAX_TIMEOUT_SECS: u32 = 600;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "HookPolicyEvent"))]
pub enum HookEvent {
    SessionStart,
    UserPromptSubmit,
    PreToolUse,
    PostToolUse,
    Stop,
}

impl HookEvent {
    #[must_use]
    pub const fn takes_tool(self) -> bool {
        matches!(self, Self::PreToolUse | Self::PostToolUse)
    }
}

/// A family of tools, named independently of any harness's tool names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    Shell,
    FileEdit,
    FileWrite,
    Subagent,
    #[default]
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HookRule {
    pub event: HookEvent,
    /// Only meaningful for tool events; ignored elsewhere.
    #[serde(default)]
    pub tool: ToolClass,
    /// Run through the harness's shell; exit 2 blocks where the event allows.
    pub command: String,
    #[serde(default)]
    pub timeout_secs: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct HookPolicy {
    #[serde(default)]
    pub rules: Vec<HookRule>,
}

/// Whether the session's harness took the policy at launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum HookPolicyStatus {
    Applied,
    /// The harness has no hook seam: the session runs unguarded by hooks.
    Unsupported,
}

/// `GET /sessions/{id}/hook-policy`: the rules sent with the session's launch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct SessionHookPolicy {
    #[serde(default)]
    pub rules: Vec<HookRule>,
    /// `None` when the launch carried no rules.
    #[serde(default)]
    pub status: Option<HookPolicyStatus>,
}

/// Harnesses whose daemon adapter converts a [`HookPolicy`] into native hooks.
#[must_use]
pub fn supports_hook_policy(adapter_id: &str) -> bool {
    matches!(adapter_id, "claude-code" | "codex")
}

/// Trim commands, drop blank ones and duplicates, and enforce the limits.
///
/// # Errors
/// A rule over a limit, with a human-readable reason.
pub fn normalize_rules(rules: Vec<HookRule>) -> Result<Vec<HookRule>, String> {
    let mut out: Vec<HookRule> = Vec::with_capacity(rules.len());
    for rule in rules {
        let command = rule.command.trim().to_owned();
        if command.is_empty() {
            continue;
        }
        if command.len() > MAX_COMMAND_BYTES {
            return Err(format!("hook command longer than {MAX_COMMAND_BYTES} bytes"));
        }
        if rule.timeout_secs.is_some_and(|t| t == 0 || t > MAX_TIMEOUT_SECS) {
            return Err(format!("hook timeout must be 1..={MAX_TIMEOUT_SECS} seconds"));
        }
        let tool = if rule.event.takes_tool() { rule.tool } else { ToolClass::Any };
        let rule = HookRule { event: rule.event, tool, command, timeout_secs: rule.timeout_secs };
        if !out.contains(&rule) {
            out.push(rule);
        }
    }
    if out.len() > MAX_RULES {
        return Err(format!("at most {MAX_RULES} hook rules"));
    }
    Ok(out)
}

/// The shim exits 0 to allow and 2 to deny; `|| exit 2` also blocks when the
/// daemon binary is missing or crashes, since harnesses fail open on other codes.
pub const NO_DIRECT_HARNESS_COMMAND: &str = "cctui-daemon hook-shim no-direct-harness || exit 2";
pub const NO_DIRECT_HARNESS_TIMEOUT_SECS: u32 = 10;

pub const DISPATCH_POLICY_FIELD: &str = "hook_policy";
pub const DISPATCH_ADDITIONS_FIELD: &str = "hook_rules";

/// A harness started from a shell would run outside the session's policy.
#[must_use]
pub fn no_direct_harness_rule() -> HookRule {
    HookRule {
        event: HookEvent::PreToolUse,
        tool: ToolClass::Shell,
        command: NO_DIRECT_HARNESS_COMMAND.to_owned(),
        timeout_secs: Some(NO_DIRECT_HARNESS_TIMEOUT_SECS),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddOnlyError {
    Weakens(HookRule),
    Invalid(String),
}

impl std::fmt::Display for AddOnlyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Weakens(rule) => {
                let timeout =
                    rule.timeout_secs.map_or_else(|| "the default".into(), |t| format!("{t}s"));
                write!(
                    f,
                    "hook rules are add-only: `{}` on {:?} is inherited with a timeout of \
                     {timeout} and cannot be restated with another one",
                    rule.command, rule.event
                )
            }
            Self::Invalid(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for AddOnlyError {}

/// The inherited rules followed by the additions.
///
/// A rule is identified by its event, tool and command. An addition with a new
/// command is a new rule even on an inherited event and tool: it runs next to
/// the inherited one and never replaces it. An addition naming an inherited
/// rule must restate it exactly; a changed timeout (which can make the hook
/// time out and fail open) is rejected, never trimmed. Inherited rules cannot
/// be omitted: they are always part of the result.
///
/// # Errors
/// [`AddOnlyError::Weakens`] for a changed inherited rule,
/// [`AddOnlyError::Invalid`] for a rule or a union over the limits.
pub fn effective_rules(
    inherited: &[HookRule],
    additions: Vec<HookRule>,
) -> Result<Vec<HookRule>, AddOnlyError> {
    let inherited = normalize_rules(inherited.to_vec()).map_err(AddOnlyError::Invalid)?;
    let additions = normalize_rules(additions).map_err(AddOnlyError::Invalid)?;
    for added in additions.iter().filter(|a| !inherited.contains(a)) {
        if let Some(parent) = inherited.iter().find(|r| same_rule(r, added)) {
            return Err(AddOnlyError::Weakens(parent.clone()));
        }
    }
    let mut all = inherited;
    all.extend(additions);
    normalize_rules(all).map_err(AddOnlyError::Invalid)
}

fn same_rule(a: &HookRule, b: &HookRule) -> bool {
    a.event == b.event && a.tool == b.tool && a.command == b.command
}

#[must_use]
pub fn from_dispatch_payload(payload: &serde_json::Value) -> Option<HookPolicy> {
    payload
        .get(DISPATCH_POLICY_FIELD)
        .and_then(|v| serde_json::from_value::<HookPolicy>(v.clone()).ok())
        .filter(|p| !p.rules.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(event: HookEvent, tool: ToolClass, command: &str) -> HookRule {
        HookRule { event, tool, command: command.into(), timeout_secs: None }
    }

    #[test]
    fn rules_use_harness_event_names_and_snake_case_tool_classes() {
        let r = HookRule {
            timeout_secs: Some(9),
            ..rule(HookEvent::PreToolUse, ToolClass::FileEdit, "x")
        };
        let json = serde_json::to_value(&r).unwrap();
        let want = serde_json::json!({
            "event": "PreToolUse", "tool": "file_edit", "command": "x", "timeout_secs": 9,
        });
        assert_eq!(json, want);
        assert_eq!(serde_json::from_value::<HookRule>(json).unwrap(), r);
    }

    #[test]
    fn an_absent_tool_means_any() {
        let r: HookRule = serde_json::from_str(r#"{"event":"Stop","command":"y"}"#).unwrap();
        assert_eq!(r, rule(HookEvent::Stop, ToolClass::Any, "y"));
    }

    #[test]
    fn normalizing_trims_dedupes_and_drops_tool_on_toolless_events() {
        let got = normalize_rules(vec![
            rule(HookEvent::PreToolUse, ToolClass::Shell, "  guard  "),
            rule(HookEvent::PreToolUse, ToolClass::Shell, "guard"),
            rule(HookEvent::Stop, ToolClass::Shell, "done"),
            rule(HookEvent::Stop, ToolClass::Any, "   "),
        ])
        .unwrap();
        assert_eq!(
            got,
            vec![
                rule(HookEvent::PreToolUse, ToolClass::Shell, "guard"),
                rule(HookEvent::Stop, ToolClass::Any, "done"),
            ]
        );
    }

    #[test]
    fn normalizing_rejects_out_of_range_timeouts() {
        let zero = HookRule { timeout_secs: Some(0), ..rule(HookEvent::Stop, ToolClass::Any, "x") };
        let huge = HookRule {
            timeout_secs: Some(MAX_TIMEOUT_SECS + 1),
            ..rule(HookEvent::Stop, ToolClass::Any, "x")
        };
        assert!(normalize_rules(vec![zero]).is_err());
        assert!(normalize_rules(vec![huge]).is_err());
    }

    #[test]
    fn claude_code_and_codex_take_a_hook_policy() {
        assert!(supports_hook_policy("claude-code"));
        assert!(supports_hook_policy("codex"));
        assert!(!supports_hook_policy("opencode"));
    }

    fn timed(command: &str, timeout_secs: Option<u32>) -> HookRule {
        HookRule { timeout_secs, ..rule(HookEvent::PreToolUse, ToolClass::Shell, command) }
    }

    #[test]
    fn an_addition_omitting_the_parent_rules_still_inherits_them() {
        let parent = vec![timed("deny-rm", Some(30))];
        let got = effective_rules(&parent, vec![timed("audit", None)]).unwrap();
        assert_eq!(got, vec![timed("deny-rm", Some(30)), timed("audit", None)]);
        assert_eq!(effective_rules(&parent, Vec::new()).unwrap(), parent);
    }

    #[test]
    fn a_new_command_on_an_inherited_event_and_tool_is_an_addition_not_a_replacement() {
        let parent = vec![timed("deny-rm", Some(30))];
        let got = effective_rules(&parent, vec![timed("allow-everything", Some(30))]).unwrap();
        assert_eq!(got, vec![timed("deny-rm", Some(30)), timed("allow-everything", Some(30))]);
    }

    #[test]
    fn restating_an_inherited_rule_exactly_is_a_no_op() {
        let parent = vec![timed("deny-rm", Some(30))];
        let got = effective_rules(&parent, vec![timed("  deny-rm ", Some(30))]).unwrap();
        assert_eq!(got, parent);
        let both = vec![timed("deny-rm", Some(30)), timed("deny-rm", Some(60))];
        assert_eq!(effective_rules(&both, vec![timed("deny-rm", Some(60))]).unwrap(), both);
    }

    #[test]
    fn restating_an_inherited_rule_with_another_timeout_is_rejected() {
        let parent = vec![timed("deny-rm", Some(30))];
        for weaker in [Some(1), None, Some(600)] {
            assert_eq!(
                effective_rules(&parent, vec![timed("deny-rm", weaker)]),
                Err(AddOnlyError::Weakens(timed("deny-rm", Some(30)))),
            );
        }
        let err = effective_rules(&parent, vec![timed("deny-rm", Some(1))]).unwrap_err();
        assert!(err.to_string().contains("add-only"), "{err}");
    }

    #[test]
    fn additions_over_the_limits_are_rejected() {
        let many: Vec<HookRule> = (0..=MAX_RULES).map(|i| timed(&format!("r{i}"), None)).collect();
        assert!(matches!(effective_rules(&[], many), Err(AddOnlyError::Invalid(_))));
        let parent: Vec<HookRule> = (0..MAX_RULES).map(|i| timed(&format!("p{i}"), None)).collect();
        assert!(matches!(
            effective_rules(&parent, vec![timed("one-more", None)]),
            Err(AddOnlyError::Invalid(_))
        ));
    }

    #[test]
    fn the_side_door_rule_guards_shell_calls_and_fails_closed() {
        let r = no_direct_harness_rule();
        assert_eq!((r.event, r.tool), (HookEvent::PreToolUse, ToolClass::Shell));
        assert!(r.command.contains("hook-shim no-direct-harness"));
        assert!(r.command.ends_with("|| exit 2"));
        assert_eq!(normalize_rules(vec![r.clone()]).unwrap(), vec![r]);
    }

    #[test]
    fn a_dispatch_payload_yields_its_written_policy() {
        let policy = HookPolicy { rules: vec![no_direct_harness_rule()] };
        let payload = serde_json::json!({ DISPATCH_POLICY_FIELD: &policy, "prompt": "go" });
        assert_eq!(from_dispatch_payload(&payload), Some(policy));
        assert_eq!(from_dispatch_payload(&serde_json::json!({ "prompt": "go" })), None);
        assert_eq!(from_dispatch_payload(&serde_json::json!({ DISPATCH_POLICY_FIELD: 7 })), None);
    }
}
