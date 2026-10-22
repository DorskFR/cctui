//! What every new agent is told before its first turn, whatever its harness.
//!
//! The content is harness-neutral; only the delivery is not. Claude-code folds
//! the block into its `<cctui-session-context>` prompt, codex passes it as
//! `developerInstructions`, opencode prepends it to the spawn prompt. Each
//! adapter therefore implements one primitive — "deliver this text to the
//! agent" — and nothing else.

use std::time::SystemTime;

use cctui_proto::api::SessionContextItem;
use cctui_proto::injected::SESSION_CONTEXT;

/// Filename the session's memory notes are staged under.
const CONTEXT_FILE: &str = "context.md";

/// Told to every session that has the cctui MCP relay: without it agents never
/// reach for the tool on their own.
pub const CONTEXT_TOOL_LINE: &str = "CctuiContext: use `mcp__cctui__CctuiContext` `save` to keep \
    a durable note for future sessions (rules you save need the user's approval); `list` and \
    `show` read what is saved, `attach` stages saved memories into this session. Never store \
    secrets in it.\n";

/// How to show the user an image; the limits are the ones [`crate::imagepost`]
/// enforces.
pub const IMAGE_LINE: &str = "images: to show the user an image, put `![alt](/absolute/path.png)` \
    in the turn's final reply, after your last tool call (text written before a tool call can be \
    dropped), or in a message to a peer session (no other tool call shows it): png, jpg, gif or \
    webp, at most 5 MB, the file under $HOME, /tmp or $TMPDIR. A posted image becomes \
    `![alt](cctui-img://…)`; to show one another session posted (a child's answer, a peer's \
    message), copy that marker into your final reply unchanged.\n";

/// The shared-checkout warning for a session about to work in `cwd`.
///
/// `None` when no other live session shares its working tree. `local_id` is
/// the session's own roster id, so a session already registered does not warn
/// about itself; `None` before the harness has minted one.
#[must_use]
pub fn shared_checkout(cwd: &str, local_id: Option<&str>) -> Option<String> {
    crate::neighbours::notice(&crate::neighbours::cwd_neighbours(cwd, local_id), SystemTime::now())
}

/// The session's rules, inline, or `None` when none apply. Rules are short by
/// construction (the server caps them), so unlike memories they are not staged
/// behind a file the agent may never open.
#[must_use]
pub fn render_rules(items: &[SessionContextItem]) -> Option<String> {
    use std::fmt::Write as _;

    let rules: Vec<&SessionContextItem> =
        items.iter().filter(|i| i.kind == "rule" && !i.body.trim().is_empty()).collect();
    if rules.is_empty() {
        return None;
    }
    let mut out = String::from("rules: the user's standing instructions; follow them.\n");
    for rule in rules {
        let _ = write!(out, "\n### {}\n{}\n", rule.title.trim(), rule.body.trim());
    }
    Some(out)
}

/// The rules, then the staged-memory notice: the one context block every
/// harness delivers. `session_id` keys the staging dir; without it the notes
/// are skipped, never the rules.
#[must_use]
pub fn context_block(session_id: Option<&str>, items: &[SessionContextItem]) -> Option<String> {
    merge(render_rules(items), session_id.and_then(|id| stage_context(id, items)))
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

/// The whole neutral preamble for one launch, in one block a harness delivers
/// verbatim.
///
/// It carries the shared-checkout warning, the session's rules and attached
/// context, and — when the session has the cctui MCP relay (`tools`) — the
/// `CctuiContext` line. A block that is sent at all also carries
/// [`IMAGE_LINE`]; the line alone is not worth a block, since a prompt-less
/// spawn would then open a turn just to deliver it.
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
    tools: bool,
) -> Option<String> {
    let context =
        merge(context_block(session_id, items), tools.then(|| CONTEXT_TOOL_LINE.to_owned()));
    let body = merge(shared_checkout(cwd, local_id), context)?;
    let body = format!("{}\n\n{IMAGE_LINE}", body.trim_end());
    Some(format!("<{SESSION_CONTEXT}>\n{}\n</{SESSION_CONTEXT}>", body.trim_end()))
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
        CONTEXT_TOOL_LINE, IMAGE_LINE, SessionContextItem, context_block, context_notice,
        for_launch, merge, render_context, render_rules, stage_context,
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
        assert!(for_launch(Some("s1"), &tmp.path().to_string_lossy(), None, &[], false).is_none());
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
        let path = crate::adapters::uploads::session_dir(&session).join("context.md");
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
        let block = for_launch(Some(&session), &tmp.path().to_string_lossy(), None, &items, false)
            .expect("a block");
        assert!(
            for_launch(None, &tmp.path().to_string_lossy(), None, &items, false).is_none(),
            "without a staging key the notes are skipped, not written blind"
        );
        assert!(block.starts_with("<cctui-session-context>\n"), "{block}");
        assert!(block.ends_with("</cctui-session-context>"), "{block}");
        assert!(block.contains("context: 1 attached"), "{block}");
        crate::adapters::uploads::remove_session_dir(&session);
    }

    #[test]
    fn rules_render_inline_and_skip_blanks_and_other_kinds() {
        let items = vec![
            item("rule", "Worktrees", "  use a worktree  "),
            item("memory", "Ops", "no prod"),
            item("rule", "Blank", "   "),
            item("prompt", "Reviewer", "review it"),
        ];
        let block = render_rules(&items).expect("a rules section");
        assert!(block.starts_with("rules:"), "{block}");
        assert!(block.contains("### Worktrees\nuse a worktree\n"), "{block}");
        assert!(!block.contains("Blank"), "{block}");
        assert!(!block.contains("no prod"), "a memory is staged, not inlined: {block}");
        assert!(!block.contains("review it"), "{block}");
        assert!(render_rules(&[item("memory", "Ops", "no prod")]).is_none());
    }

    /// One block for every harness: the rules ride inline ahead of the staged
    /// notes' pointer, and survive a launch with no staging key.
    #[test]
    fn the_context_block_puts_rules_before_the_staged_notice() {
        let session = format!("test-{}", uuid::Uuid::new_v4());
        let items = vec![item("memory", "House style", "be terse"), item("rule", "Wt", "worktree")];
        let block = context_block(Some(&session), &items).expect("a block");
        let rules = block.find("### Wt").expect("rule inline");
        let notice = block.find("context: 1 attached").expect("notice");
        assert!(rules < notice, "{block}");
        assert!(!block.contains("be terse"), "the memory body stays in the file: {block}");
        crate::adapters::uploads::remove_session_dir(&session);

        let unkeyed = context_block(None, &items).expect("rules still render");
        assert!(unkeyed.contains("### Wt"), "{unkeyed}");
        assert!(!unkeyed.contains("context:"), "{unkeyed}");
    }

    #[test]
    fn a_session_with_the_relay_is_told_about_cctui_context() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_string_lossy();
        let block =
            for_launch(None, &cwd, None, &[], true).expect("the tool line alone is a block");
        assert!(block.contains(CONTEXT_TOOL_LINE.trim_end()), "{block}");
        assert!(block.contains("need the user's approval"), "{block}");
        assert!(for_launch(None, &cwd, None, &[], false).is_none());

        let rules = vec![item("rule", "Wt", "worktree")];
        let block = for_launch(None, &cwd, None, &rules, true).expect("a block");
        assert!(
            block.find("### Wt").expect("rule") < block.find("CctuiContext").expect("line"),
            "{block}"
        );
    }

    #[test]
    fn a_sent_block_tells_the_agent_how_to_show_an_image() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path().to_string_lossy();
        let block = for_launch(None, &cwd, None, &[], true).expect("a block");
        assert!(block.contains(IMAGE_LINE.trim_end()), "{block}");
        assert!(block.ends_with(&format!("{}\n</cctui-session-context>", IMAGE_LINE.trim_end())));
        assert!(for_launch(None, &cwd, None, &[], false).is_none(), "the line alone sends nothing");
    }

    #[test]
    fn the_image_line_states_the_limits_the_daemon_enforces() {
        let mb = crate::imagepost::MAX_IMAGE_BYTES / (1024 * 1024);
        assert!(IMAGE_LINE.contains(&format!("at most {mb} MB")), "{IMAGE_LINE}");
        assert!(IMAGE_LINE.contains("![alt](/absolute/path.png)"), "{IMAGE_LINE}");
        assert!(IMAGE_LINE.contains("png, jpg, gif or webp"), "{IMAGE_LINE}");
        assert!(IMAGE_LINE.contains("in the turn's final reply, after your last tool call"));
        assert!(IMAGE_LINE.ends_with(".\n") && IMAGE_LINE.lines().count() == 1, "{IMAGE_LINE}");
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
