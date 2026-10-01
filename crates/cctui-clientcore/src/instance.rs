//! Instance status and the server's own self-update, shared with the web UI's
//! `UpdateModal` and `InstanceSection`.
//!
//! The decisions are message *ids* rather than sentences: the web UI resolves
//! them through paraglide and the TUI through [`message_text`], so the two
//! agree on which thing to say without the fixture pinning English.

use cctui_proto::updatehook::UpdateHookPhase;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseTone {
    Success,
    Faint,
    Danger,
}

impl PhaseTone {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Faint => "faint",
            Self::Danger => "danger",
        }
    }
}

/// An update is on offer only when the server reports a latest release that is
/// not the running one. The probe already withholds anything older, so a
/// present-and-different tag is enough.
#[must_use]
pub fn update_available(version: &str, latest: Option<&str>) -> bool {
    latest.is_some_and(|latest| !latest.is_empty() && latest != version)
}

/// Tone of the run readout. A run that has not reported a phase yet is treated
/// as the web UI treats it: anything but progress or success reads as a problem.
#[must_use]
pub const fn phase_tone(phase: Option<UpdateHookPhase>) -> PhaseTone {
    match phase {
        Some(UpdateHookPhase::Succeeded) => PhaseTone::Success,
        Some(UpdateHookPhase::Running | UpdateHookPhase::Verifying) => PhaseTone::Faint,
        _ => PhaseTone::Danger,
    }
}

#[must_use]
pub const fn phase_message(phase: UpdateHookPhase) -> &'static str {
    match phase {
        UpdateHookPhase::Running => "update_run_phase_running",
        UpdateHookPhase::Verifying => "update_run_phase_verifying",
        UpdateHookPhase::Succeeded => "update_run_phase_succeeded",
        UpdateHookPhase::RollingBack => "update_run_phase_rolling_back",
        UpdateHookPhase::RolledBack => "update_run_phase_rolled_back",
        UpdateHookPhase::Failed => "update_run_phase_failed",
    }
}

/// What the update surface tells this caller.
///
/// A non-admin is sent to its administrator, an admin without a configured
/// machine to the settings form, and an admin with one learns whether a hook or
/// an agent will do the work.
#[must_use]
pub const fn hint_message(is_admin: bool, ready: bool, hook: bool) -> &'static str {
    if !is_admin {
        return "update_ask_admin";
    }
    if !ready {
        return "update_no_target_hint";
    }
    if hook { "update_hook_hint" } else { "update_agent_hint" }
}

#[must_use]
pub const fn confirm_message(hook: bool) -> &'static str {
    if hook { "update_confirm_body_hook" } else { "update_confirm_body" }
}

#[must_use]
pub const fn badge_message(hook: bool) -> &'static str {
    if hook { "update_hook_badge" } else { "update_agent_badge" }
}

/// Whether an admin may launch from here at all: the trigger is admin-scoped
/// server-side and needs a configured machine, so both gates are local too.
#[must_use]
pub const fn can_launch(is_admin: bool, ready: bool, available: bool) -> bool {
    is_admin && ready && available
}

/// English for the ids above, for the clients that have no message catalogue.
#[must_use]
pub fn message_text(id: &str) -> &'static str {
    match id {
        "update_run_phase_running" => "running the update command",
        "update_run_phase_verifying" => "waiting for the new version to answer",
        "update_run_phase_succeeded" => "updated",
        "update_run_phase_rolling_back" => "rolling back",
        "update_run_phase_rolled_back" => "rolled back",
        "update_run_phase_failed" => "failed",
        "update_ask_admin" => "ask your administrator to update",
        "update_no_target_hint" => "no self-update machine is configured (set one in the web UI)",
        "update_hook_hint" => "this deployment has an update hook, so no agent is involved",
        "update_agent_hint" => "no update hook on that machine, so a YOLO agent will work it out",
        "update_hook_badge" => "deterministic update",
        "update_agent_badge" => "agent fallback",
        "update_confirm_body_hook" => {
            "the machine runs this deployment's own update command, checks that the new version \
             answers, and rolls back if it does not"
        }
        "update_confirm_body" => {
            "an agent in YOLO mode will take over the operation, with a short interruption"
        }
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PhaseTone, badge_message, can_launch, confirm_message, hint_message, message_text,
        phase_message, phase_tone, update_available,
    };
    use cctui_proto::updatehook::UpdateHookPhase;

    #[test]
    fn an_update_is_offered_only_for_a_different_tag() {
        assert!(!update_available("1.2.3", None));
        assert!(!update_available("1.2.3", Some("")));
        assert!(!update_available("1.2.3", Some("1.2.3")));
        assert!(update_available("1.2.3", Some("1.2.4")));
    }

    #[test]
    fn a_run_without_a_phase_reads_as_a_problem_like_the_web_ui() {
        assert_eq!(phase_tone(None), PhaseTone::Danger);
        assert_eq!(phase_tone(Some(UpdateHookPhase::Running)), PhaseTone::Faint);
        assert_eq!(phase_tone(Some(UpdateHookPhase::Succeeded)), PhaseTone::Success);
        assert_eq!(phase_tone(Some(UpdateHookPhase::RolledBack)), PhaseTone::Danger);
    }

    #[test]
    fn the_hint_walks_from_the_role_to_the_target_to_the_mechanism() {
        assert_eq!(hint_message(false, true, true), "update_ask_admin");
        assert_eq!(hint_message(true, false, true), "update_no_target_hint");
        assert_eq!(hint_message(true, true, true), "update_hook_hint");
        assert_eq!(hint_message(true, true, false), "update_agent_hint");
    }

    #[test]
    fn launching_needs_admin_a_target_and_something_to_install() {
        assert!(can_launch(true, true, true));
        assert!(!can_launch(false, true, true));
        assert!(!can_launch(true, false, true));
        assert!(!can_launch(true, true, false));
    }

    #[test]
    fn every_id_this_module_returns_has_english() {
        let ids = [
            phase_message(UpdateHookPhase::Running),
            phase_message(UpdateHookPhase::Verifying),
            phase_message(UpdateHookPhase::Succeeded),
            phase_message(UpdateHookPhase::RollingBack),
            phase_message(UpdateHookPhase::RolledBack),
            phase_message(UpdateHookPhase::Failed),
            hint_message(false, false, false),
            hint_message(true, false, false),
            hint_message(true, true, true),
            hint_message(true, true, false),
            confirm_message(true),
            confirm_message(false),
            badge_message(true),
            badge_message(false),
        ];
        for id in ids {
            assert!(!message_text(id).is_empty(), "no english for {id}");
        }
    }
}
