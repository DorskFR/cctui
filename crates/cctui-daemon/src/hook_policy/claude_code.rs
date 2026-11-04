//! Claude Code: policy rules become groups in the `hooks` block of the
//! per-session `--settings` file, appended after cctui's own.

use cctui_proto::hook_policy::{HookEvent, HookPolicy, HookRule, ToolClass};
use serde_json::{Value, json};

use super::{HookPolicySink, Unsupported};

pub struct ClaudeCode;

impl ClaudeCode {
    pub const ADAPTER: &'static str = "claude-code";
}

const fn event_key(event: HookEvent) -> &'static str {
    match event {
        HookEvent::SessionStart => "SessionStart",
        HookEvent::UserPromptSubmit => "UserPromptSubmit",
        HookEvent::PreToolUse => "PreToolUse",
        HookEvent::PostToolUse => "PostToolUse",
        HookEvent::Stop => "Stop",
    }
}

#[must_use]
pub const fn matcher(tool: ToolClass) -> &'static str {
    match tool {
        ToolClass::Shell => "Bash",
        ToolClass::FileEdit => "Edit|MultiEdit|NotebookEdit",
        ToolClass::FileWrite => "Write",
        ToolClass::Subagent => "Task|Agent",
        ToolClass::Any => "*",
    }
}

fn group(rule: &HookRule) -> Value {
    let mut hook = json!({ "type": "command", "command": rule.command });
    if let Some(timeout) = rule.timeout_secs {
        hook["timeout"] = json!(timeout);
    }
    let mut group = json!({ "hooks": [hook] });
    if rule.event.takes_tool() {
        group["matcher"] = json!(matcher(rule.tool));
    }
    group
}

impl HookPolicySink for ClaudeCode {
    type Launch = Value;

    fn apply(&self, policy: &HookPolicy, hooks: &mut Value) -> Result<(), Unsupported> {
        if !hooks.is_object() {
            *hooks = json!({});
        }
        for rule in &policy.rules {
            let groups = &mut hooks[event_key(rule.event)];
            if !groups.is_array() {
                *groups = json!([]);
            }
            if let Some(groups) = groups.as_array_mut() {
                groups.push(group(rule));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(event: HookEvent, tool: ToolClass, command: &str, timeout: Option<u32>) -> HookRule {
        HookRule { event, tool, command: command.into(), timeout_secs: timeout }
    }

    #[test]
    fn tool_classes_map_to_claude_matchers() {
        assert_eq!(matcher(ToolClass::Shell), "Bash");
        assert_eq!(matcher(ToolClass::FileEdit), "Edit|MultiEdit|NotebookEdit");
        assert_eq!(matcher(ToolClass::FileWrite), "Write");
        assert_eq!(matcher(ToolClass::Subagent), "Task|Agent");
        assert_eq!(matcher(ToolClass::Any), "*");
    }

    #[test]
    fn rules_append_after_existing_groups_and_only_tool_events_get_a_matcher() {
        let mut hooks = json!({ "PreToolUse": [{ "matcher": "AskUserQuestion", "hooks": [] }] });
        let policy = HookPolicy {
            rules: vec![
                rule(HookEvent::PreToolUse, ToolClass::FileWrite, "no-write", Some(7)),
                rule(HookEvent::UserPromptSubmit, ToolClass::Shell, "audit", None),
            ],
        };
        ClaudeCode.apply(&policy, &mut hooks).unwrap();
        assert_eq!(
            hooks,
            json!({
                "PreToolUse": [
                    { "matcher": "AskUserQuestion", "hooks": [] },
                    {
                        "matcher": "Write",
                        "hooks": [{ "type": "command", "command": "no-write", "timeout": 7 }],
                    },
                ],
                "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "audit" }] }],
            })
        );
    }
}
