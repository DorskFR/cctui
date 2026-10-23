//! Control frames of the live voice socket (`/api/v1/ws/voice`). Audio travels
//! as binary frames in both directions: utterances up, synthesized speech down.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

/// How long a spoken reply should be; mirrors the webui `spokenStyle` pref.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SpokenStyle {
    #[default]
    Brief,
    Normal,
    Verbose,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum VoiceState {
    Listening,
    Thinking,
    Speaking,
    /// The speech-to-text model is loading; utterances wait for it.
    Warming,
}

/// Browser → server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceClientMsg {
    /// Barge-in: stop speaking now and, when `barge_in` is on, cut the turn.
    Interrupt {},
    /// The next binary frame is a non-final part of the utterance; a binary
    /// frame without this marker ends it, so a bare binary is a whole utterance.
    Part {},
    /// The streamed utterance ended with no more audio to send.
    UtteranceEnd {},
    /// Preferences for the rest of the connection; each field keeps its
    /// previous value when absent.
    Mode {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        barge_in: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        spoken_style: Option<SpokenStyle>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        voice: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        speed: Option<f32>,
        /// Spoken language, e.g. `fr`; empty or `auto` for the instance default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
    },
}

/// Server → browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum VoiceServerMsg {
    /// What the utterance was heard as, before it goes to the session.
    Transcript {
        text: String,
    },
    State {
        state: VoiceState,
    },
    /// Binary audio frames for `line_id` follow until `speak_end`.
    SpeakStart {
        line_id: String,
        text: String,
    },
    SpeakEnd {
        line_id: String,
    },
    Error {
        message: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_use_a_type_tag() {
        let json = serde_json::to_string(&VoiceClientMsg::Interrupt {}).unwrap();
        assert_eq!(json, r#"{"type":"interrupt"}"#);
        let mode: VoiceClientMsg =
            serde_json::from_str(r#"{"type":"mode","barge_in":false,"spoken_style":"verbose"}"#)
                .unwrap();
        assert_eq!(
            mode,
            VoiceClientMsg::Mode {
                barge_in: Some(false),
                spoken_style: Some(SpokenStyle::Verbose),
                voice: None,
                speed: None,
                language: None
            }
        );
        let state =
            serde_json::to_string(&VoiceServerMsg::State { state: VoiceState::Speaking }).unwrap();
        assert_eq!(state, r#"{"type":"state","state":"speaking"}"#);
    }

    #[test]
    fn utterance_part_markers_are_bare_tags() {
        let part = serde_json::to_string(&VoiceClientMsg::Part {}).unwrap();
        assert_eq!(part, r#"{"type":"part"}"#);
        let end: VoiceClientMsg = serde_json::from_str(r#"{"type":"utterance_end"}"#).unwrap();
        assert_eq!(end, VoiceClientMsg::UtteranceEnd {});
    }
}
