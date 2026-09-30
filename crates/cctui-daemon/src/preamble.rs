//! What every new agent is told before its first turn, whatever its harness.
//!
//! The content is harness-neutral; only the delivery is not. Claude-code folds
//! the block into its `<session-context>` prompt, codex passes it as
//! `developerInstructions`, opencode prepends it to the spawn prompt. Each
//! adapter therefore implements one primitive — "deliver this text to the
//! agent" — and nothing else.

use std::time::SystemTime;

use cctui_proto::api::SessionContextItem;

/// Filename the session's memory notes are staged under.
const CONTEXT_FILE: &str = "context.md";

/// The shared-checkout warning for a session about to work in `cwd`.
///
/// `None` when no other live session shares its working tree. `local_id` is
/// the session's own roster id, so a session already registered does not warn
/// about itself; `None` before the harness has minted one.
#[must_use]
pub fn shared_checkout(cwd: &str, local_id: Option<&str>) -> Option<String> {
    crate::neighbours::notice(&crate::neighbours::cwd_neighbours(cwd, local_id), SystemTime::now())
}

/// The memory notes as one markdown document, or `None` when the session was
/// given none.
///
/// Prompt items are excluded: the server expands a prompt template into the
/// first turn, so staging it here would hand the agent its own instructions
/// twice.
#[must_use]
pub fn render_context(items: &[SessionContextItem]) -> Option<String> {
    use std::fmt::Write as _;

    let notes: Vec<&SessionContextItem> =
        items.iter().filter(|i| i.kind == "memory" && !i.body.trim().is_empty()).collect();
    if notes.is_empty() {
        return None;
    }
    let mut out = String::from("# Session context\n");
    for note in notes {
        let _ = write!(out, "\n## {}\n\n{}\n", note.title.trim(), note.body.trim());
    }
    Some(out)
}

/// The line pointing the agent at the staged notes.
#[must_use]
pub fn context_notice(path: &str, items: &[SessionContextItem]) -> String {
    let titles: Vec<&str> = items
        .iter()
        .filter(|i| i.kind == "memory" && !i.body.trim().is_empty())
        .map(|i| i.title.trim())
        .collect();
    format!(
        "context: {} attached for this session ({}). Read `{path}` before your first action and \
         follow it.\n",
        titles.len(),
        titles.join(", "),
    )
}

/// Stage the session's notes and describe them, or `None` when there are none.
///
/// A staging failure is logged and degrades to no notice: launching without
/// the notes beats refusing a session over a `/tmp` write.
#[must_use]
pub fn stage_context(session_id: &str, items: &[SessionContextItem]) -> Option<String> {
    let body = render_context(items)?;
    match crate::adapters::uploads::stage_text(session_id, CONTEXT_FILE, &body) {
        Ok(path) => Some(context_notice(&path, items)),
        Err(err) => {
            tracing::warn!(%session_id, "staging session context failed: {err:#}");
            None
        }
    }
}

/// The whole neutral preamble for one launch: the shared-checkout warning and
/// the session's attached context, in one block a harness delivers verbatim.
///
/// `session_id` keys the staging dir (the launch key, so a relaunch restages
/// in place); `None` means this launch has no stable key and its notes are
/// skipped rather than written somewhere unpredictable. `local_id` is the
/// session's roster id, absent until the harness mints one.
#[must_use]
pub fn for_launch(
    session_id: Option<&str>,
    cwd: &str,
    local_id: Option<&str>,
    items: &[SessionContextItem],
) -> Option<String> {
    let staged = session_id.and_then(|id| stage_context(id, items));
    let body = merge(shared_checkout(cwd, local_id), staged)?;
    Some(format!("<session-context>\n{}\n</session-context>", body.trim_end()))
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
    use super::{
        SessionContextItem, context_notice, for_launch, merge, render_context, stage_context,
    };

    fn item(kind: &str, title: &str, body: &str) -> SessionContextItem {
        SessionContextItem {
            kind: kind.to_owned(),
            name: title.to_ascii_lowercase(),
            title: title.to_owned(),
            body: body.to_owned(),
            version: 1,
        }
    }

    #[test]
    fn a_session_alone_with_no_context_gets_no_block() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(for_launch(Some("s1"), &tmp.path().to_string_lossy(), None, &[]).is_none());
    }

    #[test]
    fn notes_render_as_one_titled_document() {
        let items =
            vec![item("memory", "House style", "  be terse  "), item("memory", "Ops", "no prod")];
        let doc = render_context(&items).expect("a document");
        assert!(doc.starts_with("# Session context\n"), "{doc}");
        assert!(doc.contains("## House style\n\nbe terse\n"), "{doc}");
        assert!(doc.contains("## Ops\n\nno prod\n"), "{doc}");
    }

    /// The server expands a prompt template into the first turn; staging it
    /// too would hand the agent its instructions twice.
    #[test]
    fn prompt_items_and_empty_notes_are_not_staged() {
        assert!(render_context(&[item("prompt", "Reviewer", "review it")]).is_none());
        assert!(render_context(&[item("memory", "Blank", "   ")]).is_none());
        assert!(render_context(&[]).is_none());
        let mixed = vec![item("prompt", "Reviewer", "review it"), item("memory", "Keep", "this")];
        let doc = render_context(&mixed).expect("the memory survives");
        assert!(doc.contains("## Keep"), "{doc}");
        assert!(!doc.contains("Reviewer"), "{doc}");
    }

    #[test]
    fn the_notice_names_the_path_and_every_note() {
        let items = vec![item("memory", "House style", "x"), item("prompt", "Reviewer", "y")];
        let notice = context_notice("/tmp/cctui-uploads/s1/context.md", &items);
        assert!(notice.contains("1 attached"), "{notice}");
        assert!(notice.contains("House style"), "{notice}");
        assert!(!notice.contains("Reviewer"), "a template is not context: {notice}");
        assert!(notice.contains("/tmp/cctui-uploads/s1/context.md"), "{notice}");
    }

    #[test]
    fn staging_writes_the_document_and_points_at_it() {
        let session = format!("test-{}", uuid::Uuid::new_v4());
        let items = vec![item("memory", "House style", "be terse")];
        let notice = stage_context(&session, &items).expect("a notice");
        let path = std::path::Path::new("/tmp/cctui-uploads").join(&session).join("context.md");
        assert!(notice.contains(&path.to_string_lossy().into_owned()), "{notice}");
        assert!(std::fs::read_to_string(&path).unwrap().contains("be terse"));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Context alone is enough to produce a block: a session with notes and no
    /// neighbours must still be told about them.
    #[test]
    fn context_alone_produces_the_block() {
        let tmp = tempfile::tempdir().unwrap();
        let session = format!("test-{}", uuid::Uuid::new_v4());
        let items = vec![item("memory", "House style", "be terse")];
        let block = for_launch(Some(&session), &tmp.path().to_string_lossy(), None, &items)
            .expect("a block");
        assert!(
            for_launch(None, &tmp.path().to_string_lossy(), None, &items).is_none(),
            "without a staging key the notes are skipped, not written blind"
        );
        assert!(block.starts_with("<session-context>\n"), "{block}");
        assert!(block.ends_with("</session-context>"), "{block}");
        assert!(block.contains("context: 1 attached"), "{block}");
        let _ = std::fs::remove_dir_all(std::path::Path::new("/tmp/cctui-uploads").join(&session));
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
