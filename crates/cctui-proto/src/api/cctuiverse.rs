//! cctuiverse: one local session linked to one session on another cctui.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "CctuiverseLinkKind"))]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
    Session,
    Room,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "CctuiverseLinkRole"))]
#[serde(rename_all = "snake_case")]
pub enum LinkRole {
    Inviter,
    Joiner,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "CctuiverseLinkState"))]
#[serde(rename_all = "snake_case")]
pub enum LinkState {
    Pending,
    Active,
    Closed,
}

macro_rules! str_enum {
    ($ty:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        impl $ty {
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $s),+ }
            }

            #[must_use]
            pub fn parse(raw: &str) -> Option<Self> {
                match raw { $($s => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

str_enum!(LinkKind { Session => "session", Room => "room" });
str_enum!(LinkRole { Inviter => "inviter", Joiner => "joiner" });
str_enum!(LinkState { Pending => "pending", Active => "active", Closed => "closed" });
str_enum!(InboundMode { Deliver => "deliver", Hold => "hold" });
str_enum!(OutboundMode { Tool => "tool", Auto => "auto", Both => "both" });

/// What happens to a message the peer sends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "CctuiverseInbound"))]
#[serde(rename_all = "snake_case")]
pub enum InboundMode {
    #[default]
    Deliver,
    Hold,
}

/// How this side's agent reaches the peer: through `CctuiSend`, by forwarding
/// each turn's final reply, or both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "CctuiverseOutbound"))]
#[serde(rename_all = "snake_case")]
pub enum OutboundMode {
    #[default]
    Tool,
    Auto,
    Both,
}

impl OutboundMode {
    #[must_use]
    pub const fn forwards_turns(self) -> bool {
        matches!(self, Self::Auto | Self::Both)
    }
}

/// The owner's choices for one link. Every field defaults, so a partial or
/// empty stored object reads back whole.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "ts", derive(TS), ts(export, rename = "CctuiverseSettings"))]
#[serde(default)]
pub struct LinkSettings {
    pub inbound: InboundMode,
    pub outbound: OutboundMode,
    pub review_outbound: bool,
    pub share_transcript: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_messages: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CctuiverseLinkView {
    pub id: Uuid,
    pub session_id: Option<String>,
    pub room_id: Option<Uuid>,
    pub kind: LinkKind,
    pub role: LinkRole,
    pub state: LinkState,
    pub label: String,
    pub peer_label: Option<String>,
    /// Host name of the peer's server only.
    pub peer_host: Option<String>,
    pub peer_room_name: Option<String>,
    /// `xxxx-xxxx-xxxx-xxxx`, derived from both public keys; `None` until active.
    pub safety_code: Option<String>,
    pub settings: LinkSettings,
    pub sent_count: i32,
    pub held_count: i64,
    pub review_count: i64,
    pub invite_expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub activated_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// A held inbound or review-pending outbound message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct CctuiverseMessageView {
    pub id: i64,
    pub message_id: Uuid,
    pub direction: String,
    pub kind: String,
    pub text: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_settings_read_back_as_defaults() {
        let s: LinkSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, LinkSettings::default());
        assert_eq!(s.inbound, InboundMode::Deliver);
        assert_eq!(s.outbound, OutboundMode::Tool);
        assert!(!s.review_outbound && !s.share_transcript);
        assert!(s.expires_at.is_none() && s.max_messages.is_none());
    }

    #[test]
    fn settings_wire_shape_is_snake_case() {
        let s = LinkSettings {
            inbound: InboundMode::Hold,
            outbound: OutboundMode::Both,
            max_messages: Some(3),
            ..LinkSettings::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["inbound"], "hold");
        assert_eq!(v["outbound"], "both");
        assert_eq!(v["max_messages"], 3);
        assert!(v["expires_at"].is_null());
        assert!(OutboundMode::Auto.forwards_turns() && !OutboundMode::Tool.forwards_turns());
    }

    #[test]
    fn enum_strings_round_trip() {
        for k in [LinkKind::Session, LinkKind::Room] {
            assert_eq!(LinkKind::parse(k.as_str()), Some(k));
        }
        for s in [LinkState::Pending, LinkState::Active, LinkState::Closed] {
            assert_eq!(LinkState::parse(s.as_str()), Some(s));
        }
        assert_eq!(LinkRole::parse("joiner"), Some(LinkRole::Joiner));
        assert_eq!(LinkRole::parse("Joiner"), None);
    }
}
