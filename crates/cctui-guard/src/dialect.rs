//! Harness dialects for `/check` verdicts.
//!
//! The engine decides in its own vocabulary; this module renders that decision
//! in the shape the calling harness's pre-tool hook expects.

use serde_json::{Value, json};

use crate::engine::Verdict;

/// Header a caller may use instead of the `?dialect=` query parameter.
pub const DIALECT_HEADER: &str = "x-guard-dialect";

/// The response shape a harness expects from a pre-tool hook.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// `{"decision":"allow"|"deny"|"ask","reason":"…"}`.
    Native,
    /// Claude Code `PreToolUse` `hookSpecificOutput`. The default: deployed
    /// context-pack hooks send no dialect and must keep receiving this shape.
    #[default]
    Claude,
    /// Codex hook decision: `allow` / `block` / `ask`.
    Codex,
    /// `OpenCode` `tool.execute.before` shim: `{"allow":bool,"reason":"…"}`.
    OpenCode,
    /// ACP `session/request_permission` outcome with the chosen option id.
    Acp,
}

impl Dialect {
    /// Parse a dialect name.
    ///
    /// Unknown or empty names are `None`; the caller falls back to the default
    /// so a typo never changes the verdict shape to something the harness
    /// cannot read.
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.trim().to_ascii_lowercase().as_str() {
            "native" => Some(Self::Native),
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            "opencode" => Some(Self::OpenCode),
            "acp" => Some(Self::Acp),
            _ => None,
        }
    }

    /// Resolve the dialect from the query parameter, then the header, then
    /// the default.
    #[must_use]
    pub fn resolve(query: Option<&str>, header: Option<&str>) -> Self {
        query.and_then(Self::parse).or_else(|| header.and_then(Self::parse)).unwrap_or_default()
    }

    /// Render a verdict in this dialect.
    #[must_use]
    pub fn render(self, verdict: &Verdict) -> Value {
        match self {
            Self::Native => match verdict {
                Verdict::Allow => json!({ "decision": "allow" }),
                Verdict::Deny { reason } => json!({ "decision": "deny", "reason": reason }),
                Verdict::Ask { reason } => json!({ "decision": "ask", "reason": reason }),
            },
            Self::Claude => match verdict {
                Verdict::Allow => json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "allow",
                    }
                }),
                Verdict::Deny { reason } => json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "deny",
                        "permissionDecisionReason": reason,
                    }
                }),
                Verdict::Ask { reason } => json!({
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecision": "ask",
                        "permissionDecisionReason": reason,
                    }
                }),
            },
            Self::Codex => match verdict {
                Verdict::Allow => json!({ "decision": "allow" }),
                Verdict::Deny { reason } => json!({ "decision": "block", "reason": reason }),
                Verdict::Ask { reason } => json!({ "decision": "ask", "reason": reason }),
            },
            Self::OpenCode => match verdict {
                Verdict::Allow => json!({ "allow": true }),
                Verdict::Deny { reason } => json!({ "allow": false, "reason": reason }),
                Verdict::Ask { reason } => json!({ "allow": false, "ask": true, "reason": reason }),
            },
            Self::Acp => match verdict {
                Verdict::Allow => json!({
                    "outcome": { "outcome": "selected", "optionId": "allow_once" }
                }),
                Verdict::Deny { reason } => json!({
                    "outcome": { "outcome": "selected", "optionId": "reject_once" },
                    "_meta": { "reason": reason }
                }),
                Verdict::Ask { reason } => json!({
                    "outcome": { "outcome": "cancelled" },
                    "_meta": { "reason": reason }
                }),
            },
        }
    }

    /// A deny rendered for the paths where the engine never produced a
    /// verdict (panicked task, unreadable body). Never fails open.
    #[must_use]
    pub fn fail_closed(self, reason: &str) -> Value {
        self.render(&Verdict::deny(reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compact(v: &Value) -> String {
        serde_json::to_string(v).unwrap()
    }

    #[test]
    fn parse_is_case_insensitive_and_rejects_unknown() {
        assert_eq!(Dialect::parse("Native"), Some(Dialect::Native));
        assert_eq!(Dialect::parse(" codex "), Some(Dialect::Codex));
        assert_eq!(Dialect::parse("OpenCode"), Some(Dialect::OpenCode));
        assert_eq!(Dialect::parse("acp"), Some(Dialect::Acp));
        assert_eq!(Dialect::parse("gemini"), None);
        assert_eq!(Dialect::parse(""), None);
    }

    #[test]
    fn resolve_prefers_query_then_header_then_claude() {
        assert_eq!(Dialect::resolve(Some("native"), Some("codex")), Dialect::Native);
        assert_eq!(Dialect::resolve(Some("bogus"), Some("codex")), Dialect::Codex);
        assert_eq!(Dialect::resolve(None, Some("acp")), Dialect::Acp);
        assert_eq!(Dialect::resolve(None, None), Dialect::Claude);
        assert_eq!(Dialect::resolve(Some("bogus"), None), Dialect::Claude);
    }

    #[test]
    fn native_snapshots() {
        insta::assert_snapshot!(
            compact(&Dialect::Native.render(&Verdict::Allow)),
            @r#"{"decision":"allow"}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Native.render(&Verdict::deny("[Step 2] no"))),
            @r#"{"decision":"deny","reason":"[Step 2] no"}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Native.fail_closed("internal guard error")),
            @r#"{"decision":"deny","reason":"internal guard error"}"#
        );
    }

    #[test]
    fn claude_snapshots() {
        insta::assert_snapshot!(
            compact(&Dialect::Claude.render(&Verdict::Allow)),
            @r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"allow"}}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Claude.render(&Verdict::deny("[Step 2] no"))),
            @r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"[Step 2] no"}}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Claude.fail_closed("internal guard error")),
            @r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"internal guard error"}}"#
        );
    }

    #[test]
    fn codex_snapshots() {
        insta::assert_snapshot!(
            compact(&Dialect::Codex.render(&Verdict::Allow)),
            @r#"{"decision":"allow"}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Codex.render(&Verdict::deny("[Step 2] no"))),
            @r#"{"decision":"block","reason":"[Step 2] no"}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Codex.fail_closed("internal guard error")),
            @r#"{"decision":"block","reason":"internal guard error"}"#
        );
    }

    #[test]
    fn opencode_snapshots() {
        insta::assert_snapshot!(
            compact(&Dialect::OpenCode.render(&Verdict::Allow)),
            @r#"{"allow":true}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::OpenCode.render(&Verdict::deny("[Step 2] no"))),
            @r#"{"allow":false,"reason":"[Step 2] no"}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::OpenCode.fail_closed("internal guard error")),
            @r#"{"allow":false,"reason":"internal guard error"}"#
        );
    }

    #[test]
    fn acp_snapshots() {
        insta::assert_snapshot!(
            compact(&Dialect::Acp.render(&Verdict::Allow)),
            @r#"{"outcome":{"optionId":"allow_once","outcome":"selected"}}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Acp.render(&Verdict::deny("[Step 2] no"))),
            @r#"{"_meta":{"reason":"[Step 2] no"},"outcome":{"optionId":"reject_once","outcome":"selected"}}"#
        );
        insta::assert_snapshot!(
            compact(&Dialect::Acp.fail_closed("internal guard error")),
            @r#"{"_meta":{"reason":"internal guard error"},"outcome":{"optionId":"reject_once","outcome":"selected"}}"#
        );
    }

    #[test]
    fn claude_dialect_is_byte_identical_to_the_legacy_hook_response() {
        let legacy_allow = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "allow",
            }
        });
        let legacy_deny = json!({
            "hookSpecificOutput": {
                "hookEventName": "PreToolUse",
                "permissionDecision": "deny",
                "permissionDecisionReason": "[Step 2] 'git push' is disallowed in this step",
            }
        });
        assert_eq!(compact(&Dialect::Claude.render(&Verdict::Allow)), compact(&legacy_allow));
        assert_eq!(
            compact(
                &Dialect::Claude
                    .render(&Verdict::deny("[Step 2] 'git push' is disallowed in this step"))
            ),
            compact(&legacy_deny)
        );
    }

    #[test]
    fn every_dialect_fails_closed_as_a_deny() {
        for d in [Dialect::Native, Dialect::Claude, Dialect::Codex, Dialect::OpenCode, Dialect::Acp]
        {
            assert_eq!(d.fail_closed("x"), d.render(&Verdict::deny("x")));
            assert_ne!(d.fail_closed("x"), d.render(&Verdict::Allow));
        }
    }
}
