//! Converts cctui's harness-neutral [`HookPolicy`] into each harness's native
//! hook configuration at launch.

pub mod claude_code;
pub mod codex;
pub mod no_direct_harness;
pub mod shim;

use cctui_proto::hook_policy::{HookPolicy, HookRule};

const DAEMON_TOKEN: &str = "cctui-daemon ";
pub const UNRESOLVED_DAEMON: &str =
    "echo 'cctui: the cctui-daemon binary is unresolved, this hook blocks' >&2; exit 2";

/// This daemon's binary, as rules calling `cctui-daemon` must run it.
#[must_use]
pub fn daemon_exe() -> Option<String> {
    crate::selfupdate::install_path().map(|p| p.to_string_lossy().into_owned())
}

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Point every rule that calls `cctui-daemon` at `exe`, so a daemon that is
/// not on `PATH` still answers it. Without an `exe` such a rule blocks.
#[must_use]
pub fn materialize(policy: &HookPolicy, exe: Option<&str>) -> HookPolicy {
    let rules = policy
        .rules
        .iter()
        .map(|rule| {
            let Some(rest) = rule.command.strip_prefix(DAEMON_TOKEN) else {
                return rule.clone();
            };
            let command = exe.map_or_else(
                || {
                    tracing::error!(
                        command = %rule.command,
                        "cannot resolve the cctui-daemon binary: this hook rule blocks every call \
                         it matches"
                    );
                    UNRESOLVED_DAEMON.to_owned()
                },
                |exe| format!("{} {rest}", shell_quote(exe)),
            );
            HookRule { command, ..rule.clone() }
        })
        .collect();
    HookPolicy { rules }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    pub adapter: String,
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} has no hook seam: the session runs unguarded by hooks", self.adapter)
    }
}

impl std::error::Error for Unsupported {}

pub trait HookPolicySink {
    type Launch;

    /// Add the policy's hooks to `launch`, keeping every hook already there.
    ///
    /// # Errors
    /// [`Unsupported`] when the harness cannot take hooks.
    fn apply(&self, policy: &HookPolicy, launch: &mut Self::Launch) -> Result<(), Unsupported>;
}

pub struct NoSeam(pub &'static str);

impl HookPolicySink for NoSeam {
    type Launch = ();

    fn apply(&self, _: &HookPolicy, &mut (): &mut ()) -> Result<(), Unsupported> {
        Err(Unsupported { adapter: self.0.to_owned() })
    }
}

pub const OPENCODE: NoSeam = NoSeam("opencode");

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::hook_policy::{HookEvent, HookRule, ToolClass, supports_hook_policy};

    fn policy() -> HookPolicy {
        HookPolicy {
            rules: vec![HookRule {
                event: HookEvent::PreToolUse,
                tool: ToolClass::Shell,
                command: "deny".into(),
                timeout_secs: None,
            }],
        }
    }

    #[test]
    fn harnesses_without_a_seam_report_unsupported() {
        assert_eq!(
            OPENCODE.apply(&policy(), &mut ()),
            Err(Unsupported { adapter: OPENCODE.0.to_owned() })
        );
        assert!(!supports_hook_policy(OPENCODE.0), "the server must record opencode unguarded");
    }

    #[test]
    fn claude_code_takes_the_policy_as_the_server_expects() {
        let mut hooks = serde_json::json!({});
        assert_eq!(claude_code::ClaudeCode.apply(&policy(), &mut hooks), Ok(()));
        assert!(supports_hook_policy(claude_code::ClaudeCode::ADAPTER));
    }

    #[test]
    fn daemon_rules_run_this_daemons_binary() {
        let side_door = cctui_proto::hook_policy::no_direct_harness_rule();
        let policy = HookPolicy { rules: vec![side_door.clone(), policy().rules[0].clone()] };
        let got = materialize(&policy, Some("/opt/cctui it's/cctui-daemon"));
        assert_eq!(
            got.rules[0].command,
            r"'/opt/cctui it'\''s/cctui-daemon' hook-shim no-direct-harness || exit 2"
        );
        assert_eq!(got.rules[0].timeout_secs, side_door.timeout_secs);
        assert_eq!(got.rules[1], policy.rules[1], "other rules are untouched");
        let lookalike = HookRule { command: "cctui-daemonx run".into(), ..side_door };
        let unchanged = HookPolicy { rules: vec![lookalike] };
        assert_eq!(materialize(&unchanged, Some("/x")), unchanged);
    }

    #[test]
    fn an_unresolved_daemon_binary_keeps_the_rule_blocking() {
        let policy = HookPolicy { rules: vec![cctui_proto::hook_policy::no_direct_harness_rule()] };
        let got = materialize(&policy, None);
        assert_eq!(got.rules[0].command, UNRESOLVED_DAEMON);
        assert!(got.rules[0].command.ends_with("exit 2"));
    }

    #[test]
    fn both_sinks_hand_the_harness_the_daemons_own_binary() {
        let policy = HookPolicy { rules: vec![cctui_proto::hook_policy::no_direct_harness_rule()] };
        let mut hooks = serde_json::json!({});
        claude_code::ClaudeCode.apply(&policy, &mut hooks).unwrap();
        let claude = hooks["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().to_owned();
        let exe = daemon_exe().expect("the test binary resolves");
        assert!(claude.starts_with(&shell_quote(&exe)), "{claude}");

        let mut config = serde_json::json!({});
        codex::Codex { exe: Some("/d/cctui-daemon".into()) }.apply(&policy, &mut config).unwrap();
        assert_eq!(
            config["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            "'/d/cctui-daemon' hook-shim no-direct-harness || exit 2"
        );
        let mut config = serde_json::json!({});
        codex::Codex { exe: None }.apply(&policy, &mut config).unwrap();
        assert_eq!(config["hooks"]["PreToolUse"][0]["hooks"][0]["command"], UNRESOLVED_DAEMON);
    }

    #[test]
    fn codex_takes_the_policy_as_the_server_expects() {
        let mut config = serde_json::json!({});
        assert_eq!(codex::Codex::current().apply(&policy(), &mut config), Ok(()));
        assert_eq!(config["bypass_hook_trust"], true);
        assert!(supports_hook_policy(codex::Codex::ADAPTER));
    }
}
