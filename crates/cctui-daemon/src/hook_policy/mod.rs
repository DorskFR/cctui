//! Converts cctui's harness-neutral [`HookPolicy`] into each harness's native
//! hook configuration at launch.

pub mod claude_code;
pub mod codex;
pub mod shim;

use cctui_proto::hook_policy::HookPolicy;

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

    fn apply(&self, _: &HookPolicy, _: &mut ()) -> Result<(), Unsupported> {
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
    fn codex_takes_the_policy_as_the_server_expects() {
        let mut config = serde_json::json!({});
        assert_eq!(codex::Codex::current().apply(&policy(), &mut config), Ok(()));
        assert_eq!(config["bypass_hook_trust"], true);
        assert!(supports_hook_policy(codex::Codex::ADAPTER));
    }
}
