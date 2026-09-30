//! How an ended session is presented: colour tier, muting, badge detail.
//!
//! The rules belong to the domain, not to either client; the wording of the
//! label stays client-side.

use serde::{Deserialize, Serialize};
#[cfg(feature = "ts")]
use ts_rs::TS;

use crate::models::SessionEndReason;

/// Colour tier for an end badge.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum EndTone {
    #[default]
    Neutral,
    Ok,
    Warn,
    Danger,
    Info,
}

/// How much of a failed start's detail the badge carries.
pub const BADGE_DETAIL_MAX: usize = 48;

/// One row of the end-reason table served to clients, so neither of them
/// hardcodes the tone rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(TS), ts(export))]
pub struct EndReasonInfo {
    pub reason: SessionEndReason,
    pub tone: EndTone,
    pub muted: bool,
    pub failed_start: bool,
}

/// Every end reason with its derivations, in enum order.
#[must_use]
pub fn end_reason_table() -> Vec<EndReasonInfo> {
    [
        SessionEndReason::Completed,
        SessionEndReason::Killed,
        SessionEndReason::Crashed,
        SessionEndReason::DaemonLost,
        SessionEndReason::MachineOffline,
        SessionEndReason::ReapedInactive,
        SessionEndReason::ResumeFailed,
        SessionEndReason::SpawnFailed,
        SessionEndReason::Other,
    ]
    .into_iter()
    .map(|reason| EndReasonInfo {
        reason,
        tone: reason.tone(),
        muted: reason.muted(),
        failed_start: reason.failed_start(),
    })
    .collect()
}

impl SessionEndReason {
    #[must_use]
    pub const fn tone(self) -> EndTone {
        match self {
            Self::Completed => EndTone::Ok,
            Self::Crashed | Self::ResumeFailed | Self::SpawnFailed => EndTone::Danger,
            Self::DaemonLost | Self::MachineOffline => EndTone::Warn,
            _ => EndTone::Neutral,
        }
    }

    /// Reaped sessions aged out silently — rendered faded, not as a state.
    #[must_use]
    pub const fn muted(self) -> bool {
        matches!(self, Self::ReapedInactive)
    }

    /// A start that never produced a session: its detail is the whole story,
    /// so the badge carries the first line of it.
    #[must_use]
    pub const fn failed_start(self) -> bool {
        matches!(self, Self::ResumeFailed | Self::SpawnFailed)
    }
}

/// The first line of a failed start's detail, ellipsised to
/// [`BADGE_DETAIL_MAX`]. `None` when the badge is just the label.
#[must_use]
pub fn end_badge_detail(reason: SessionEndReason, detail: Option<&str>) -> Option<String> {
    if !reason.failed_start() {
        return None;
    }
    let line = detail?.split('\n').next()?.trim();
    if line.is_empty() {
        return None;
    }
    if line.chars().count() > BADGE_DETAIL_MAX {
        let short: String = line.chars().take(BADGE_DETAIL_MAX - 1).collect();
        return Some(format!("{short}…"));
    }
    Some(line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_reason_maps_to_its_colour() {
        assert_eq!(SessionEndReason::Completed.tone(), EndTone::Ok);
        assert_eq!(SessionEndReason::Killed.tone(), EndTone::Neutral);
        assert_eq!(SessionEndReason::Crashed.tone(), EndTone::Danger);
        assert_eq!(SessionEndReason::ResumeFailed.tone(), EndTone::Danger);
        assert_eq!(SessionEndReason::SpawnFailed.tone(), EndTone::Danger);
        assert_eq!(SessionEndReason::DaemonLost.tone(), EndTone::Warn);
        assert_eq!(SessionEndReason::MachineOffline.tone(), EndTone::Warn);
        assert_eq!(SessionEndReason::ReapedInactive.tone(), EndTone::Neutral);
        assert_eq!(SessionEndReason::Other.tone(), EndTone::Neutral);
    }

    #[test]
    fn only_a_reaped_session_is_muted() {
        assert!(SessionEndReason::ReapedInactive.muted());
        assert!(!SessionEndReason::Crashed.muted());
    }

    #[test]
    fn the_badge_carries_the_first_line_of_a_failed_start_truncated() {
        assert_eq!(
            end_badge_detail(
                SessionEndReason::SpawnFailed,
                Some("unknown model gpt-nope; available: gpt-5-codex\nsecond line")
            )
            .as_deref(),
            Some("unknown model gpt-nope; available: gpt-5-codex")
        );
        let long = "a".repeat(60);
        assert_eq!(
            end_badge_detail(SessionEndReason::SpawnFailed, Some(&format!("{long}\nb"))).as_deref(),
            Some(format!("{}…", "a".repeat(BADGE_DETAIL_MAX - 1)).as_str())
        );
        assert_eq!(end_badge_detail(SessionEndReason::Crashed, Some("boom")), None);
        assert_eq!(end_badge_detail(SessionEndReason::ResumeFailed, None), None);
        assert_eq!(
            end_badge_detail(SessionEndReason::ResumeFailed, Some("auth")).as_deref(),
            Some("auth")
        );
    }
}
