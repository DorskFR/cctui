//! What `y`, `Y` and the link key put on the clipboard. Pure: the transport is
//! [`super::action::Effect::Copy`]'s job.

use super::state::{ConversationLine, LineKind, ToolCategory};

/// The focused line as a Markdown block, in the same shape the export writes so
/// a pasted line and a pasted transcript read alike.
#[must_use]
pub fn line_markdown(line: &ConversationLine) -> String {
    let body = &line.text;
    match line.kind {
        LineKind::User | LineKind::Reply => format!("**User:**\n\n{body}"),
        LineKind::Assistant => format!("**Assistant:**\n\n{body}"),
        LineKind::Thinking { redacted: false } => format!("**Thinking:**\n\n{body}"),
        LineKind::Thinking { redacted: true } => "**Thinking:** _(redacted)_".to_owned(),
        LineKind::Tool { category } => {
            let tool = line.tool.as_deref().unwrap_or("tool");
            let lang = if category == ToolCategory::Mcp { "json" } else { "" };
            format!("**Tool · {tool}**\n\n{}", fence(body, lang))
        }
        LineKind::Result { error } => {
            let tool = line.tool.as_deref().unwrap_or("tool");
            let label = if error { "Result (failed)" } else { "Result" };
            format!("**{label} · {tool}**\n\n{}", fence(body, ""))
        }
        LineKind::Peer => {
            let who = line
                .peer_from
                .as_deref()
                .map_or_else(|| "Peer".to_owned(), |from| format!("Peer · {from}"));
            format!("**{who}:**\n\n{body}")
        }
        LineKind::Compact => format!("**Compacted context:**\n\n{body}"),
        LineKind::Marker | LineKind::Reset | LineKind::Summary | LineKind::System => {
            format!("_{body}_")
        }
    }
}

fn fence(body: &str, lang: &str) -> String {
    let safe = body.replace("```", "` ` `");
    format!("```{lang}\n{safe}\n```")
}

/// The code under the cursor, unwrapped and unlabelled — what you paste into an
/// editor. A tool call or result *is* code; prose has to carry a fence.
#[must_use]
pub fn code_block(line: &ConversationLine) -> Option<String> {
    if matches!(line.kind, LineKind::Tool { .. } | LineKind::Result { .. }) {
        let body = line.text.trim();
        return (!body.is_empty()).then(|| body.to_owned());
    }
    first_fenced(&line.text)
}

/// The first fenced body in `text`, without its ` ``` ` lines.
fn first_fenced(text: &str) -> Option<String> {
    let mut body: Vec<&str> = Vec::new();
    let mut inside = false;
    for raw in text.lines() {
        let line = raw.trim_start();
        if line.starts_with("```") {
            if inside {
                return Some(body.join("\n"));
            }
            inside = true;
            continue;
        }
        if inside {
            body.push(raw);
        }
    }
    // An unterminated fence still holds the code the reader is looking at.
    (inside && !body.is_empty()).then(|| body.join("\n"))
}

/// The webui's own share link, so a pasted URL opens the same page.
#[must_use]
pub fn session_link(base_url: &str, session_id: &str) -> String {
    format!("{}/sessions?session={session_id}", base_url.trim_end_matches('/'))
}

/// `ESC ] 52 ; c ; <base64> BEL`. Terminals that support it copy to the system
/// clipboard even across ssh; the ones that do not print nothing visible, which
/// is why a local fallback still runs.
#[must_use]
pub fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        let idx = [(n >> 18) & 63, (n >> 12) & 63, (n >> 6) & 63, n & 63];
        out.push(B64[idx[0] as usize] as char);
        out.push(B64[idx[1] as usize] as char);
        out.push(if chunk.len() > 1 { B64[idx[2] as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { B64[idx[3] as usize] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{base64, code_block, line_markdown, osc52, session_link};
    use crate::app::state::{ConversationLine, LineKind, ToolCategory};

    fn line(kind: LineKind, text: &str) -> ConversationLine {
        ConversationLine::new(kind, text, 0)
    }

    fn tool(name: &str, text: &str) -> ConversationLine {
        let mut l = line(LineKind::Tool { category: ToolCategory::Write }, text);
        l.tool = Some(name.to_owned());
        l
    }

    #[test]
    fn each_kind_copies_with_the_label_the_export_uses() {
        assert_eq!(line_markdown(&line(LineKind::User, "ship it")), "**User:**\n\nship it");
        assert_eq!(line_markdown(&line(LineKind::Reply, "ok")), "**User:**\n\nok");
        assert_eq!(line_markdown(&line(LineKind::Assistant, "done")), "**Assistant:**\n\ndone");
        assert_eq!(
            line_markdown(&line(LineKind::Thinking { redacted: false }, "hmm")),
            "**Thinking:**\n\nhmm"
        );
        assert_eq!(
            line_markdown(&tool("Bash", "cargo test")),
            "**Tool · Bash**\n\n```\ncargo test\n```"
        );
        assert_eq!(line_markdown(&line(LineKind::Marker, "tick")), "_tick_");
    }

    #[test]
    fn a_failed_result_says_so_in_its_label() {
        let mut failed = line(LineKind::Result { error: true }, "exit 1");
        failed.tool = Some("Bash".to_owned());
        assert_eq!(line_markdown(&failed), "**Result (failed) · Bash**\n\n```\nexit 1\n```");
    }

    #[test]
    fn a_peer_line_names_its_sender() {
        let mut peer = line(LineKind::Peer, "rebased");
        peer.peer_from = Some("lane-b".to_owned());
        assert_eq!(line_markdown(&peer), "**Peer · lane-b:**\n\nrebased");
        peer.peer_from = None;
        assert_eq!(line_markdown(&peer), "**Peer:**\n\nrebased");
    }

    #[test]
    fn a_copied_body_cannot_break_out_of_its_fence() {
        let copied = line_markdown(&tool("Write", "```rust\nfn x() {}\n```"));
        assert!(copied.contains("` ` `rust"));
        assert_eq!(copied.matches("```").count(), 2);
    }

    #[test]
    fn a_tool_line_is_code_on_its_own() {
        assert_eq!(code_block(&tool("Bash", "cargo test")).as_deref(), Some("cargo test"));
        assert!(code_block(&tool("Bash", "   ")).is_none());
    }

    #[test]
    fn prose_yields_the_first_fenced_block_only() {
        let prose = line(
            LineKind::Assistant,
            "try this:\n\n```rust\nfn a() {}\n```\n\nand then:\n\n```sh\nls\n```",
        );
        assert_eq!(code_block(&prose).as_deref(), Some("fn a() {}"));
        assert!(code_block(&line(LineKind::Assistant, "no code here")).is_none());
    }

    #[test]
    fn an_unterminated_fence_still_copies_what_it_holds() {
        let cut_off = line(LineKind::Assistant, "see:\n```py\nprint(1)\nprint(2)");
        assert_eq!(code_block(&cut_off).as_deref(), Some("print(1)\nprint(2)"));
    }

    #[test]
    fn a_session_link_matches_the_webui_route() {
        assert_eq!(
            session_link("https://cctui.dorsk.dev/", "s-1"),
            "https://cctui.dorsk.dev/sessions?session=s-1"
        );
    }

    #[test]
    fn base64_pads_every_tail_length() {
        assert_eq!(base64(b"a"), "YQ==");
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(base64(b"hello world"), "aGVsbG8gd29ybGQ=");
        assert_eq!(base64(b""), "");
    }

    #[test]
    fn the_osc52_frame_wraps_the_payload() {
        assert_eq!(osc52("abc"), "\x1b]52;c;YWJj\x07");
    }
}
