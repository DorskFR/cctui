//! What every new agent is told before its first turn, whatever its harness.
//!
//! The content is harness-neutral; only the delivery is not. Claude-code folds
//! the block into its `<session-context>` prompt, codex passes it as
//! `developerInstructions`, opencode prepends it to the spawn prompt. Each
//! adapter therefore implements one primitive — "deliver this text to the
//! agent" — and nothing else.

use std::time::SystemTime;

/// The shared-checkout warning for a session about to work in `cwd`.
///
/// `None` when no other live session shares its working tree. `local_id` is
/// the session's own roster id, so a session already registered does not warn
/// about itself; `None` before the harness has minted one.
#[must_use]
pub fn shared_checkout(cwd: &str, local_id: Option<&str>) -> Option<String> {
    crate::neighbours::notice(&crate::neighbours::cwd_neighbours(cwd, local_id), SystemTime::now())
}

/// The preamble as a standalone block, for a harness that has no session
/// context of its own to fold it into.
#[must_use]
pub fn block(cwd: &str, local_id: Option<&str>) -> Option<String> {
    let notice = shared_checkout(cwd, local_id)?;
    Some(format!("<session-context>\n{}\n</session-context>", notice.trim_end()))
}

/// `base` with `extra` appended, skipping whichever is absent or blank.
#[must_use]
pub fn merge(base: Option<String>, extra: Option<String>) -> Option<String> {
    let keep = |s: Option<String>| s.filter(|t| !t.trim().is_empty());
    match (keep(base), keep(extra)) {
        (Some(a), Some(b)) => Some(format!("{a}\n\n{b}")),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::{block, merge};

    #[test]
    fn a_session_alone_in_its_tree_gets_no_block() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(block(&tmp.path().to_string_lossy(), None).is_none());
    }

    #[test]
    fn merging_keeps_whichever_side_has_content() {
        assert_eq!(merge(Some("a".into()), Some("b".into())).as_deref(), Some("a\n\nb"));
        assert_eq!(merge(Some("a".into()), None).as_deref(), Some("a"));
        assert_eq!(merge(None, Some("b".into())).as_deref(), Some("b"));
        assert_eq!(merge(Some("  ".into()), Some("b".into())).as_deref(), Some("b"));
        assert_eq!(merge(None, None), None);
    }
}
