use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum FollowupReason {
    Cold,
    Manual,
    Auto,
    Inbox,
}

impl FollowupReason {
    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "cold" => Some(Self::Cold),
            "manual" => Some(Self::Manual),
            "auto" => Some(Self::Auto),
            "inbox" => Some(Self::Inbox),
            _ => None,
        }
    }
}

/// A block cctui injected into a generation's first turn. The history above
/// it already shows that content, so clients hide it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum InjectedFlag {
    PreviousSessions,
    SessionContext,
}

/// One line of a collated history. `generation` is 1-based, the root is 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "entry", rename_all = "snake_case")]
pub enum ThreadEntry {
    Event {
        session_id: String,
        generation: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        injected: Option<InjectedFlag>,
        event: serde_json::Value,
    },
    FollowupMarker {
        generation: u32,
        from: String,
        to: String,
        at: chrono::DateTime<chrono::Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<FollowupReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instruction: Option<String>,
    },
    Unreadable {
        session_id: String,
        generation: u32,
    },
}

impl ThreadEntry {
    #[must_use]
    pub const fn generation(&self) -> u32 {
        match self {
            Self::Event { generation, .. }
            | Self::FollowupMarker { generation, .. }
            | Self::Unreadable { generation, .. } => *generation,
        }
    }

    /// Where this entry sorts: pass it as a cursor to read what came before it.
    #[must_use]
    pub fn cursor(&self) -> ThreadCursor {
        let seq = match self {
            Self::Event { event, .. } => {
                event.get("seq").and_then(serde_json::Value::as_i64).unwrap_or_default()
            }
            Self::FollowupMarker { .. } => ThreadCursor::MARKER_SEQ,
            Self::Unreadable { .. } => ThreadCursor::PLACEHOLDER_SEQ,
        };
        ThreadCursor { generation: self.generation(), seq }
    }
}

/// A generation of the line; `name` is withheld when the caller cannot read it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ThreadGeneration {
    pub session_id: String,
    pub generation: u32,
    pub readable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub registered_at: chrono::DateTime<chrono::Utc>,
}

/// A page of `GET /sessions/{id}/conversation?thread=1`.
///
/// The newest entries of the line root → `head`, oldest first. Pass
/// `next_cursor` as `cursor` for the page before it; `None` at the root's
/// first event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct ThreadConversationPage {
    pub thread_root_id: String,
    pub head: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_session_id: Option<String>,
    pub generations: Vec<ThreadGeneration>,
    pub entries: Vec<ThreadEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Where a page of a collated history stops.
///
/// Entries strictly before `(generation, seq)`; a generation's marker sorts at
/// seq `-1` and an unreadable placeholder at `0`, before its events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ThreadCursor {
    pub generation: u32,
    pub seq: i64,
}

impl ThreadCursor {
    pub const MARKER_SEQ: i64 = -1;
    pub const PLACEHOLDER_SEQ: i64 = 0;

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        let (generation, seq) = raw.trim().split_once(':')?;
        let generation = generation.parse::<u32>().ok().filter(|g| *g >= 1)?;
        let seq = seq.parse::<i64>().ok().filter(|s| *s >= Self::MARKER_SEQ)?;
        Some(Self { generation, seq })
    }
}

impl std::fmt::Display for ThreadCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.generation, self.seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cursor_round_trips_and_rejects_garbage() {
        let c = ThreadCursor { generation: 3, seq: 4_242 };
        assert_eq!(ThreadCursor::parse(&c.to_string()), Some(c));
        assert_eq!(
            ThreadCursor::parse("2:-1"),
            Some(ThreadCursor { generation: 2, seq: ThreadCursor::MARKER_SEQ })
        );
        for bad in ["", "3", "0:5", "x:1", "2:-2", "2:y", "-1:3"] {
            assert_eq!(ThreadCursor::parse(bad), None, "{bad}");
        }
        assert!(ThreadCursor { generation: 2, seq: 9 } < ThreadCursor { generation: 3, seq: -1 });
    }

    #[test]
    fn entries_are_tagged_by_kind_and_omit_absent_fields() {
        let event = ThreadEntry::Event {
            session_id: "b".into(),
            generation: 2,
            injected: Some(InjectedFlag::PreviousSessions),
            event: serde_json::json!({"type": "text", "seq": 7}),
        };
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({"entry": "event", "session_id": "b", "generation": 2,
                "injected": "previous_sessions", "event": {"type": "text", "seq": 7}})
        );
        let at = chrono::DateTime::from_timestamp(0, 0).unwrap();
        let marker = ThreadEntry::FollowupMarker {
            generation: 2,
            from: "a".into(),
            to: "b".into(),
            at,
            reason: None,
            instruction: None,
        };
        let v = serde_json::to_value(&marker).unwrap();
        assert_eq!(v["entry"], "followup_marker");
        assert!(v.get("reason").is_none() && v.get("instruction").is_none(), "{v}");
        assert_eq!(marker.cursor(), ThreadCursor { generation: 2, seq: ThreadCursor::MARKER_SEQ });
        assert_eq!(event.cursor(), ThreadCursor { generation: 2, seq: 7 });
        assert_eq!(FollowupReason::parse("inbox"), Some(FollowupReason::Inbox));
        assert_eq!(FollowupReason::parse("later"), None);
    }
}
