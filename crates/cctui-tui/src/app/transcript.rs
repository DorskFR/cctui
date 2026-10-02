//! Pure classification of transcript text; the webui counterpart is
//! `conversation/format.ts` and the two must agree.

use std::sync::OnceLock;

use regex::Regex;

/// How history stores a user turn.
pub const USER_PREFIX: &str = "▷ User:";

/// Must match the server's `keepalive::TICK_MARKER`.
pub const TICK_MARKER: &str = "[cctui keep-alive";

/// Markers the harness prefixes onto text it injects rather than text a human
/// typed. Kept in sync with `META_TAGS` in the webui and `META_MARKERS` in the
/// daemon's transcript parser.
const META_TAGS: &[&str] = &[
    "<task-notification",
    "<system-reminder",
    "<command-name",
    "<command-message",
    "<local-command",
    "<bash-input",
    "<bash-stdout",
    "<bash-stderr",
    "[SYSTEM NOTIFICATION",
    "Base directory for this skill:",
    "Stop hook feedback:",
    "# Autonomous loop",
];

/// A marker counts only in the head of a turn: the harness prefixes at most its
/// own sentence before what it injects, while a human pasting a transcript
/// quotes markers arbitrarily deep and must stay a human turn.
const META_HEAD_LINES: usize = 4;

/// The harness prefixes its own sentence before the peer wrapper, so the tag is
/// never at the start of the turn: the preamble is what identifies the shape.
const PEER_PREAMBLES: &[&str] = &[
    "Another Claude session sent a message:",
    "Another session sent a message:",
    "Received a message from agent",
];

#[must_use]
pub fn looks_keepalive_tick(text: &str) -> bool {
    text.trim_start().starts_with(TICK_MARKER)
}

#[must_use]
pub fn looks_meta(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(META_HEAD_LINES)
        .any(|line| META_TAGS.iter().any(|tag| line.starts_with(tag)))
}

pub struct PeerMessage {
    /// `from-name` when the sender supplied one, else the raw `from` address.
    pub from: Option<String>,
    /// The room a post came through; absent for a direct peer message.
    pub room: Option<String>,
    pub body: String,
}

fn peer_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?m)^<(cross-session-message|agent-message|cctui-room)([^>]*)>([\s\S]*?)</(?:cross-session-message|agent-message|cctui-room)>",
        )
        .expect("the peer tag pattern compiles")
    })
}

fn room_joined_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?m)^<cctui-room-joined[^>]*>([\s\S]*?)</cctui-room-joined>")
            .expect("the room-joined pattern compiles")
    })
}

fn attr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"([a-z-]+)="([^"]*)""#).expect("the attribute pattern compiles"))
}

fn is_peer_preamble(line: &str) -> bool {
    let trimmed = line.trim();
    PEER_PREAMBLES.iter().any(|p| trimmed.starts_with(p))
}

/// True when everything before the wrapper is harness preamble. A human quoting
/// or relaying a wrapper writes prose around it and stays a human turn.
fn only_preamble_before(text: &str, at: usize) -> bool {
    text[..at].lines().all(|l| l.trim().is_empty() || is_peer_preamble(l))
}

fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    attr_re()
        .captures_iter(attrs)
        .find(|c| c.get(1).is_some_and(|m| m.as_str() == name))
        .and_then(|c| c.get(2).map(|m| m.as_str()))
        .map(str::trim)
        .filter(|v| !v.is_empty())
}

#[must_use]
pub fn parse_peer_message(text: &str) -> Option<PeerMessage> {
    let caps = peer_tag_re().captures(text)?;
    let whole = caps.get(0)?;
    if !only_preamble_before(text, whole.start()) {
        return None;
    }
    let tag = caps.get(1)?.as_str();
    // `cctui-room-joined` also starts with `cctui-room`; it is a system marker,
    // not a peer message. The close tag is an alternation rather than a
    // backreference (regex-lite has none), so it is checked here.
    if text[whole.start()..].starts_with("<cctui-room-joined")
        || !whole.as_str().ends_with(&format!("</{tag}>"))
    {
        return None;
    }
    let attrs = caps.get(2).map_or("", |m| m.as_str());
    let room = (tag == "cctui-room").then(|| attr(attrs, "name")).flatten();
    Some(PeerMessage {
        from: attr(attrs, "from-name").or_else(|| attr(attrs, "from")).map(str::to_owned),
        room: room.map(str::to_owned),
        body: caps.get(3).map_or("", |m| m.as_str()).trim().to_owned(),
    })
}

/// The body of a room-join notice, or `None` when this is not one. The notice is
/// addressed to the agent rather than sent by a peer, so it reads as a marker.
#[must_use]
pub fn parse_room_joined(text: &str) -> Option<String> {
    let caps = room_joined_re().captures(text)?;
    if !only_preamble_before(text, caps.get(0)?.start()) {
        return None;
    }
    Some(caps.get(1)?.as_str().trim().to_owned())
}

/// A `turn_annotation` payload: `<kind>[:<detail>]`.
#[must_use]
pub fn parse_annotation(content: &str) -> (&str, &str) {
    content.split_once(':').unwrap_or((content, ""))
}

/// Shortens `mcp__server__tool` to `server:tool`; other names pass through.
#[must_use]
pub fn display_tool_name(tool: &str) -> String {
    tool.strip_prefix("mcp__").map_or_else(|| tool.to_owned(), |rest| rest.replacen("__", ":", 1))
}

#[cfg(test)]
mod tests {
    use super::{
        looks_keepalive_tick, looks_meta, parse_annotation, parse_peer_message, parse_room_joined,
    };

    #[test]
    fn a_harness_marker_in_the_head_of_a_turn_is_meta() {
        assert!(looks_meta("<system-reminder>be good</system-reminder>"));
        assert!(looks_meta("Stop hook feedback:\nfix the lint"));
        assert!(!looks_meta("refactor the parser"));
    }

    #[test]
    fn a_marker_quoted_deep_in_a_human_turn_is_not_meta() {
        let text = "one\ntwo\nthree\nfour\nfive\n<system-reminder>quoted</system-reminder>";
        assert!(!looks_meta(text));
        assert!(!looks_meta("I saw a <system-reminder> in the log"));
    }

    #[test]
    fn a_peer_wrapper_behind_its_preamble_is_a_peer_message() {
        let text = "Another Claude session sent a message:\n<cross-session-message from=\"a-1\" from-name=\"worker\">check the tests</cross-session-message>";
        let peer = parse_peer_message(text).expect("a peer message");
        assert_eq!(peer.from.as_deref(), Some("worker"));
        assert_eq!(peer.body, "check the tests");
        assert!(peer.room.is_none());
    }

    #[test]
    fn a_from_address_stands_in_when_no_name_was_supplied() {
        let text = "<agent-message from=\"a-99\">done</agent-message>";
        let peer = parse_peer_message(text).expect("a peer message");
        assert_eq!(peer.from.as_deref(), Some("a-99"));
    }

    #[test]
    fn a_room_post_carries_the_room_it_came_through() {
        let text = "<cctui-room name=\"wave-3\" from-name=\"lane-a\">integrating</cctui-room>";
        let peer = parse_peer_message(text).expect("a peer message");
        assert_eq!(peer.room.as_deref(), Some("wave-3"));
        assert_eq!(peer.from.as_deref(), Some("lane-a"));
    }

    #[test]
    fn a_human_relaying_a_wrapper_stays_a_human_turn() {
        let text = "look at this:\n<cross-session-message from=\"a-1\">hi</cross-session-message>";
        assert!(parse_peer_message(text).is_none());
    }

    #[test]
    fn a_room_join_notice_is_not_a_peer_message() {
        let text = "<cctui-room-joined name=\"wave-3\">you are in wave-3</cctui-room-joined>";
        assert!(parse_peer_message(text).is_none());
        assert_eq!(parse_room_joined(text).as_deref(), Some("you are in wave-3"));
    }

    #[test]
    fn a_mismatched_close_tag_is_not_a_peer_message() {
        let text = "<agent-message from=\"a-1\">hi</cctui-room>";
        assert!(parse_peer_message(text).is_none());
    }

    #[test]
    fn a_keepalive_tick_is_recognised_through_leading_space() {
        assert!(looks_keepalive_tick("  [cctui keep-alive 3/6]"));
        assert!(!looks_keepalive_tick("the [cctui keep-alive] marker"));
    }

    #[test]
    fn an_annotation_splits_on_its_first_colon() {
        assert_eq!(parse_annotation("turn_duration:38000"), ("turn_duration", "38000"));
        assert_eq!(parse_annotation("stop_hook_summary:a:b"), ("stop_hook_summary", "a:b"));
        assert_eq!(parse_annotation("bare"), ("bare", ""));
    }

    #[test]
    fn an_mcp_tool_name_is_shortened_to_server_and_tool() {
        assert_eq!(super::display_tool_name("mcp__cctui__CctuiAgent"), "cctui:CctuiAgent");
        assert_eq!(super::display_tool_name("Read"), "Read");
    }
}
