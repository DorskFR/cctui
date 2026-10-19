//! The wrappers cctui injects into agent prompts, and the splitter that tells
//! them apart from the human's words. Keep the tag names in sync with the
//! webui's list in `webui/src/lib/components/organisms/conversation/format.ts`.
//!
//! Stored history is immutable, so readers accept the legacy names forever.

pub const SESSION_CONTEXT: &str = "cctui-session-context";
pub const PREVIOUS_CONVERSATION: &str = "cctui-previous-conversation";
pub const PEER_MESSAGE: &str = "cctui-peer-message";
pub const ROOM: &str = "cctui-room";
pub const ROOM_JOINED: &str = "cctui-room-joined";

pub const LEGACY_SESSION_CONTEXT: &str = "session-context";
pub const LEGACY_PREVIOUS_CONVERSATION: &str = "previous-conversation";
pub const LEGACY_PEER_MESSAGE: &str = "cross-session-message";

pub const PEER_TAGS: [&str; 2] = [PEER_MESSAGE, LEGACY_PEER_MESSAGE];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InjectedKind {
    SessionContext,
    PreviousConversation,
}

impl InjectedKind {
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            SESSION_CONTEXT | LEGACY_SESSION_CONTEXT => Some(Self::SessionContext),
            PREVIOUS_CONVERSATION | LEGACY_PREVIOUS_CONVERSATION => {
                Some(Self::PreviousConversation)
            }
            _ => None,
        }
    }
}

/// One wrapper leading a turn: its tag as written, raw attributes and body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InjectedBlock<'a> {
    pub kind: InjectedKind,
    pub tag: &'a str,
    pub attrs: &'a str,
    pub body: &'a str,
}

impl InjectedBlock<'_> {
    /// The entity-decoded value of attribute `name`.
    #[must_use]
    pub fn attr(&self, name: &str) -> Option<String> {
        attr_value(self.attrs, name)
    }
}

/// The value of `name="…"` in a tag's attribute text, entity-decoded.
#[must_use]
pub fn attr_value(attrs: &str, name: &str) -> Option<String> {
    let mut rest = attrs;
    loop {
        let at = rest.find(name)?;
        let starts_word = rest[..at].chars().next_back().is_none_or(char::is_whitespace);
        let after = &rest[at + name.len()..];
        if starts_word && let Some(value) = after.strip_prefix("=\"") {
            let end = value.find('"')?;
            return Some(decode_entities(&value[..end]));
        }
        rest = after;
    }
}

fn decode_entities(raw: &str) -> String {
    raw.replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&#39;", "'")
        .replace("&amp;", "&")
}

/// Whether `text` opens on a direct peer envelope, under either name.
#[must_use]
pub fn starts_with_peer_tag(text: &str) -> bool {
    PEER_TAGS.iter().any(|tag| opens_tag(text, tag))
}

fn opens_tag(text: &str, tag: &str) -> bool {
    text.strip_prefix('<')
        .and_then(|r| r.strip_prefix(tag))
        .is_some_and(|r| r.starts_with(|c: char| c == '>' || c.is_whitespace()))
}

/// Split the wrappers leading a turn from the prose after them.
///
/// Only consecutive blocks at the head count, separated by whitespace or the
/// legacy `===` line; a tag quoted mid-message is prose. With no leading block
/// the content comes back untouched.
#[must_use]
pub fn split_injected(content: &str) -> (Vec<InjectedBlock<'_>>, &str) {
    let mut blocks = Vec::new();
    let mut rest = content;
    while let Some((block, after)) = parse_block(skip_separators(rest)) {
        blocks.push(block);
        rest = after;
    }
    if blocks.is_empty() { (blocks, content) } else { (blocks, skip_separators(rest)) }
}

fn skip_separators(mut s: &str) -> &str {
    loop {
        let trimmed = s.trim_start();
        let Some(after) = trimmed.strip_prefix("===") else { return trimmed };
        let line_end = after.find('\n').unwrap_or(after.len());
        if !after[..line_end].trim().is_empty() {
            return trimmed;
        }
        s = &after[line_end..];
    }
}

fn parse_block(s: &str) -> Option<(InjectedBlock<'_>, &str)> {
    let after_lt = s.strip_prefix('<')?;
    let name_len =
        after_lt.find(|c: char| !(c.is_ascii_lowercase() || c == '-')).unwrap_or(after_lt.len());
    let tag = &after_lt[..name_len];
    let kind = InjectedKind::from_tag(tag)?;
    let after_name = &after_lt[name_len..];
    let gt = after_name.find('>')?;
    let attrs = &after_name[..gt];
    if !attrs.is_empty() && !attrs.starts_with(char::is_whitespace) {
        return None;
    }
    let inner = &after_name[gt + 1..];
    let close = format!("</{tag}>");
    let end = balanced_close(inner, tag, &close).or_else(|| inner.rfind(&close))?;
    let block = InjectedBlock { kind, tag, attrs: attrs.trim(), body: inner[..end].trim() };
    Some((block, &inner[end + close.len()..]))
}

/// The offset of the `close` that matches the already-open `tag`, counting
/// nested blocks of the same tag (a legacy follow-up of a follow-up).
fn balanced_close(s: &str, tag: &str, close: &str) -> Option<usize> {
    let mut depth = 0usize;
    for (i, _) in s.match_indices('<') {
        let rest = &s[i..];
        if rest.starts_with(close) {
            if depth == 0 {
                return Some(i);
            }
            depth -= 1;
        } else if opens_tag(rest, tag) {
            depth += 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(content: &str) -> (Vec<(InjectedKind, &str)>, &str) {
        let (blocks, prose) = split_injected(content);
        (blocks.iter().map(|b| (b.kind, b.tag)).collect(), prose)
    }

    #[test]
    fn the_splitter_table() {
        use InjectedKind::{PreviousConversation as Prev, SessionContext as Ctx};
        let cases: &[(&str, &str, &[(InjectedKind, &str)], &str)] = &[
            (
                "leading new name",
                "<cctui-session-context>\nsession: a\n</cctui-session-context>\n\nfix the bug",
                &[(Ctx, SESSION_CONTEXT)],
                "fix the bug",
            ),
            (
                "leading legacy name",
                "<session-context>\ncwd: /w\n</session-context>\n\nfix the bug",
                &[(Ctx, LEGACY_SESSION_CONTEXT)],
                "fix the bug",
            ),
            (
                "quoted mid-text stays prose",
                "look at this:\n<session-context>\ncwd: /w\n</session-context>",
                &[],
                "look at this:\n<session-context>\ncwd: /w\n</session-context>",
            ),
            (
                "legacy follow-up with === separator",
                "<session-context>\nx\n</session-context>\n\n<previous-conversation session=\"a\" \
                 id=\"s1\">\n**User:**\n\nq\n</previous-conversation>\n\n===\n\nNow add tests.",
                &[(Ctx, LEGACY_SESSION_CONTEXT), (Prev, LEGACY_PREVIOUS_CONVERSATION)],
                "Now add tests.",
            ),
            (
                "new follow-up without separator",
                "<cctui-session-context>\nx\n</cctui-session-context>\n\n\
                 <cctui-previous-conversation session=\"a\" id=\"s1\">\nbrief\n\
                 </cctui-previous-conversation>\n\nContinue from this.",
                &[(Ctx, SESSION_CONTEXT), (Prev, PREVIOUS_CONVERSATION)],
                "Continue from this.",
            ),
            (
                "no prose",
                "<cctui-session-context>\nx\n</cctui-session-context>\n",
                &[(Ctx, SESSION_CONTEXT)],
                "",
            ),
            ("plain prose", "hello", &[], "hello"),
            (
                "an unclosed wrapper is prose",
                "<session-context>\nnever closed",
                &[],
                "<session-context>\nnever closed",
            ),
            (
                "a lookalike tag is prose",
                "<session-contextual>x</session-contextual> hi",
                &[],
                "<session-contextual>x</session-contextual> hi",
            ),
            (
                "a leading === alone is prose",
                "===\nheading",
                &[],
                "===\nheading",
            ),
            (
                "peer envelopes are not context",
                "<cctui-peer-message from=\"s1\">\nhi\n</cctui-peer-message>",
                &[],
                "<cctui-peer-message from=\"s1\">\nhi\n</cctui-peer-message>",
            ),
        ];
        for (name, content, want_blocks, want_prose) in cases {
            let (blocks, prose) = kinds(content);
            assert_eq!(blocks.as_slice(), *want_blocks, "{name}");
            assert_eq!(prose, *want_prose, "{name}");
        }
    }

    #[test]
    fn a_nested_legacy_follow_up_is_one_block() {
        let content = "<previous-conversation session=\"b\" id=\"s2\">\n**User:**\n\n\
                       <previous-conversation session=\"a\" id=\"s1\">\nold\n\
                       </previous-conversation>\n\n===\n\nmore\n</previous-conversation>\n\n\
                       ===\n\nask";
        let (blocks, prose) = split_injected(content);
        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].body.ends_with("more"), "{}", blocks[0].body);
        assert_eq!(blocks[0].attr("id").as_deref(), Some("s2"));
        assert_eq!(prose, "ask");
    }

    #[test]
    fn attributes_are_read_whole_word_and_decoded() {
        let (blocks, _) = split_injected(
            "<cctui-previous-conversation session=\"my &quot;run&quot; &amp; co\" id=\"s1\">\nb\n\
             </cctui-previous-conversation>",
        );
        assert_eq!(blocks[0].attr("session").as_deref(), Some("my \"run\" & co"));
        assert_eq!(blocks[0].attr("id").as_deref(), Some("s1"));
        assert_eq!(attr_value("from-name=\"x\" from=\"s1\"", "from").as_deref(), Some("s1"));
        assert_eq!(attr_value("session_id=\"x\"", "id"), None);
    }

    #[test]
    fn peer_tags_under_either_name() {
        assert!(starts_with_peer_tag("<cctui-peer-message from=\"s\">"));
        assert!(starts_with_peer_tag("<cross-session-message from=\"s\">"));
        assert!(!starts_with_peer_tag("<cctui-peer-messages>"));
        assert!(!starts_with_peer_tag("see <cctui-peer-message>"));
    }
}
