//! Codex: policy rules become the `hooks` block of the `thread/start`,
//! `thread/resume` and `thread/fork` `params.config`.
//!
//! Trust: `bypass_hook_trust` is set on the thread. Codex snapshots hooks per
//! thread, so the bypass also lets the user's and the repo's untrusted
//! `hooks.json` run in that thread. Accepted: those hooks only add checks
//! (and run code the agent could already run through its shell tool), while
//! pre-seeding `hooks.state.<key>.trusted_hash` for cctui's hooks alone would
//! depend on Codex's private hashing.
//!
//! `features.hooks = true` (a dotted key, so it cannot replace the whole
//! `features` table) rides along because a trusted repo's or the user's
//! `features.hooks = false` otherwise drops every hook, injected ones included.

pub mod interactive;

use cctui_proto::hook_policy::{HookEvent, HookPolicy, HookRule, ToolClass};
use serde_json::{Value, json};

use super::shim::Tools;
use super::{HookPolicySink, Unsupported};

/// Rides along with every shell guard: commands typed into an interactive
/// session through `write_stdin` never reach `PreToolUse`.
pub const NO_INTERACTIVE_SHELL_COMMAND: &str =
    "cctui-daemon hook-shim no-interactive-shell || exit 2";
const NO_INTERACTIVE_SHELL_TIMEOUT_SECS: u32 = 10;

fn with_interactive_refusal(policy: &HookPolicy) -> HookPolicy {
    let mut policy = policy.clone();
    let guards_shell = policy.rules.iter().any(|r| {
        r.event == HookEvent::PreToolUse && matches!(r.tool, ToolClass::Shell | ToolClass::Any)
    });
    if guards_shell && !policy.rules.iter().any(|r| r.command == NO_INTERACTIVE_SHELL_COMMAND) {
        policy.rules.push(HookRule {
            event: HookEvent::PreToolUse,
            tool: ToolClass::Shell,
            command: NO_INTERACTIVE_SHELL_COMMAND.to_owned(),
            timeout_secs: Some(NO_INTERACTIVE_SHELL_TIMEOUT_SECS),
        });
    }
    policy
}

pub struct Codex {
    /// The daemon binary the `hook-shim` runs from. `None`: rules that need
    /// the shim block every call they match instead.
    pub exe: Option<String>,
}

impl Codex {
    pub const ADAPTER: &'static str = "codex";

    #[must_use]
    pub fn current() -> Self {
        Self { exe: std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned()) }
    }
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
        ToolClass::FileEdit | ToolClass::FileWrite => "apply_patch",
        ToolClass::Subagent => "spawn_agent",
        ToolClass::Any => "*",
    }
}

const fn shim_tools(tool: ToolClass) -> Option<Tools> {
    match tool {
        ToolClass::FileEdit => Some(Tools::Edit),
        ToolClass::FileWrite => Some(Tools::Write),
        ToolClass::Any => Some(Tools::Any),
        ToolClass::Shell | ToolClass::Subagent => None,
    }
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

impl Codex {
    fn command(&self, rule: &HookRule) -> String {
        let tools = rule.event.takes_tool().then(|| shim_tools(rule.tool)).flatten();
        let Some(tools) = tools else { return rule.command.clone() };
        self.exe.as_deref().map_or_else(
            || "echo 'cctui: apply_patch hook shim unavailable' >&2; exit 2".to_owned(),
            |exe| {
                format!(
                    "{} hook-shim apply-patch --tools {} --run {}",
                    shell_quote(exe),
                    tools.as_arg(),
                    shell_quote(&rule.command)
                )
            },
        )
    }

    fn group(&self, rule: &HookRule) -> Value {
        let mut hook = json!({ "type": "command", "command": self.command(rule) });
        if let Some(timeout) = rule.timeout_secs {
            hook["timeout"] = json!(timeout);
        }
        let mut group = json!({ "hooks": [hook] });
        if rule.event.takes_tool() {
            group["matcher"] = json!(matcher(rule.tool));
        }
        group
    }
}

fn object(slot: &mut Value) -> &mut serde_json::Map<String, Value> {
    if !slot.is_object() {
        *slot = json!({});
    }
    slot.as_object_mut().unwrap_or_else(|| unreachable!())
}

impl HookPolicySink for Codex {
    /// The thread request's `params.config` object.
    type Launch = Value;

    fn apply(&self, policy: &HookPolicy, config: &mut Value) -> Result<(), Unsupported> {
        if policy.rules.is_empty() {
            return Ok(());
        }
        let policy = super::materialize(&with_interactive_refusal(policy), self.exe.as_deref());
        let config = object(config);
        let hooks = object(config.entry("hooks").or_insert_with(|| json!({})));
        for rule in &policy.rules {
            let groups = hooks.entry(event_key(rule.event)).or_insert_with(|| json!([]));
            if !groups.is_array() {
                *groups = json!([]);
            }
            if let Some(groups) = groups.as_array_mut() {
                groups.push(self.group(rule));
            }
        }
        if let Some(features) = config.get_mut("features").and_then(Value::as_object_mut) {
            features.insert("hooks".to_owned(), json!(true));
        }
        config.insert("features.hooks".to_owned(), json!(true));
        config.insert("bypass_hook_trust".to_owned(), json!(true));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(event: HookEvent, tool: ToolClass, command: &str) -> HookRule {
        HookRule { event, tool, command: command.into(), timeout_secs: None }
    }

    fn sink() -> Codex {
        Codex { exe: Some("/opt/cctui daemon".into()) }
    }

    #[test]
    fn tool_classes_map_to_codex_matchers() {
        assert_eq!(matcher(ToolClass::Shell), "Bash");
        assert_eq!(matcher(ToolClass::FileEdit), "apply_patch");
        assert_eq!(matcher(ToolClass::FileWrite), "apply_patch");
        assert_eq!(matcher(ToolClass::Subagent), "spawn_agent");
        assert_eq!(matcher(ToolClass::Any), "*");
    }

    #[test]
    fn an_empty_policy_leaves_the_config_alone() {
        let mut config = json!({ "service_tier": "fast" });
        sink().apply(&HookPolicy::default(), &mut config).unwrap();
        assert_eq!(config, json!({ "service_tier": "fast" }));
    }

    #[test]
    fn rules_become_hooks_with_features_on_and_trust_bypassed() {
        let mut config = json!({ "service_tier": "fast", "features": { "web_search": true } });
        let policy = HookPolicy {
            rules: vec![
                HookRule {
                    timeout_secs: Some(7),
                    ..rule(HookEvent::PreToolUse, ToolClass::Shell, "deny-rm")
                },
                rule(HookEvent::UserPromptSubmit, ToolClass::Shell, "audit"),
            ],
        };
        sink().apply(&policy, &mut config).unwrap();
        assert_eq!(
            config,
            json!({
                "service_tier": "fast",
                "features": { "web_search": true, "hooks": true },
                "features.hooks": true,
                "bypass_hook_trust": true,
                "hooks": {
                    "PreToolUse": [
                        {
                            "matcher": "Bash",
                            "hooks": [{ "type": "command", "command": "deny-rm", "timeout": 7 }],
                        },
                        {
                            "matcher": "Bash",
                            "hooks": [{
                                "type": "command",
                                "command":
                                    "'/opt/cctui daemon' hook-shim no-interactive-shell || exit 2",
                                "timeout": 10,
                            }],
                        },
                    ],
                    "UserPromptSubmit": [{ "hooks": [{ "type": "command", "command": "audit" }] }],
                },
            })
        );
    }

    #[test]
    fn file_and_any_rules_run_through_the_apply_patch_shim() {
        let mut config = json!({});
        let policy = HookPolicy {
            rules: vec![
                rule(HookEvent::PreToolUse, ToolClass::FileEdit, "guard 'x'"),
                rule(HookEvent::PreToolUse, ToolClass::FileWrite, "w"),
                rule(HookEvent::PostToolUse, ToolClass::Any, "a"),
                rule(HookEvent::PreToolUse, ToolClass::Subagent, "s"),
            ],
        };
        sink().apply(&policy, &mut config).unwrap();
        let pre = &config["hooks"]["PreToolUse"];
        assert_eq!(pre[0]["matcher"], "apply_patch");
        assert_eq!(
            pre[0]["hooks"][0]["command"],
            r"'/opt/cctui daemon' hook-shim apply-patch --tools edit --run 'guard '\''x'\'''"
        );
        assert_eq!(
            pre[1]["hooks"][0]["command"],
            "'/opt/cctui daemon' hook-shim apply-patch --tools write --run 'w'"
        );
        assert_eq!(pre[2]["matcher"], "spawn_agent");
        assert_eq!(pre[2]["hooks"][0]["command"], "s");
        let post = &config["hooks"]["PostToolUse"][0];
        assert_eq!(post["matcher"], "*");
        assert_eq!(
            post["hooks"][0]["command"],
            "'/opt/cctui daemon' hook-shim apply-patch --tools any --run 'a'"
        );
    }

    fn pre_tool_commands(config: &Value) -> Vec<String> {
        config["hooks"]["PreToolUse"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|g| g["hooks"][0]["command"].as_str())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn a_shell_guard_brings_the_interactive_session_refusal_once() {
        for tool in [ToolClass::Shell, ToolClass::Any] {
            let mut config = json!({});
            let policy = HookPolicy { rules: vec![rule(HookEvent::PreToolUse, tool, "guard")] };
            sink().apply(&policy, &mut config).unwrap();
            let refusal = &config["hooks"]["PreToolUse"][1];
            assert_eq!(refusal["matcher"], "Bash", "{config}");
            assert_eq!(
                refusal["hooks"][0]["command"],
                "'/opt/cctui daemon' hook-shim no-interactive-shell || exit 2"
            );
            assert_eq!(refusal["hooks"][0]["timeout"], 10);
        }

        let already = HookPolicy {
            rules: vec![
                rule(HookEvent::PreToolUse, ToolClass::Shell, "guard"),
                HookRule {
                    timeout_secs: Some(10),
                    ..rule(HookEvent::PreToolUse, ToolClass::Shell, NO_INTERACTIVE_SHELL_COMMAND)
                },
            ],
        };
        let mut config = json!({});
        sink().apply(&already, &mut config).unwrap();
        let refusals = pre_tool_commands(&config)
            .into_iter()
            .filter(|c| c.contains("no-interactive-shell"))
            .count();
        assert_eq!(refusals, 1, "{config}");
    }

    #[test]
    fn no_shell_guard_means_no_interactive_session_refusal() {
        let mut config = json!({});
        let policy = HookPolicy {
            rules: vec![
                rule(HookEvent::PreToolUse, ToolClass::FileEdit, "g"),
                rule(HookEvent::PostToolUse, ToolClass::Shell, "audit"),
            ],
        };
        sink().apply(&policy, &mut config).unwrap();
        assert!(!config.to_string().contains("no-interactive-shell"), "{config}");
    }

    #[test]
    fn without_the_daemon_binary_the_interactive_refusal_blocks() {
        let mut config = json!({});
        let policy = HookPolicy { rules: vec![rule(HookEvent::PreToolUse, ToolClass::Shell, "g")] };
        Codex { exe: None }.apply(&policy, &mut config).unwrap();
        assert_eq!(pre_tool_commands(&config), ["g", super::super::UNRESOLVED_DAEMON]);
    }

    #[test]
    fn the_shim_cli_takes_no_interactive_shell() {
        use clap::Parser as _;
        #[derive(clap::Parser)]
        struct Cli {
            #[command(subcommand)]
            shim: super::super::shim::ShimCmd,
        }
        let cli = Cli::try_parse_from(["hook-shim", "no-interactive-shell"]).unwrap();
        assert!(matches!(cli.shim, super::super::shim::ShimCmd::NoInteractiveShell));
    }

    #[test]
    fn without_the_shim_file_rules_block_instead_of_running_unguarded() {
        let mut config = json!({});
        let policy =
            HookPolicy { rules: vec![rule(HookEvent::PreToolUse, ToolClass::FileEdit, "g")] };
        Codex { exe: None }.apply(&policy, &mut config).unwrap();
        let command = config["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(command.ends_with("exit 2"), "{command}");
    }

    #[test]
    fn the_sink_never_writes_an_ask_decision() {
        let mut config = json!({});
        let policy = HookPolicy {
            rules: [ToolClass::Shell, ToolClass::FileEdit, ToolClass::FileWrite, ToolClass::Any]
                .into_iter()
                .map(|tool| rule(HookEvent::PreToolUse, tool, "guard"))
                .collect(),
        };
        sink().apply(&policy, &mut config).unwrap();
        assert!(!config.to_string().contains("ask"), "{config}");
    }
}
