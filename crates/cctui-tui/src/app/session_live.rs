//! Live session-list updates: websocket events edit rows in place, and the
//! REST refresh becomes a slow safety net rather than the source of truth.

use cctui_proto::models::{Liveness, MachineLiveness, SessionEndReason};

use super::action::Effect;
use super::state::App;

/// Two refreshes closer together than this collapse into one.
pub const REFRESH_DEBOUNCE_MS: i64 = 2_000;

/// Poll period while the socket is delivering events.
pub const POLL_HEALTHY_MS: i64 = 15_000;

/// Poll period with no live socket: the refresh is the only source of truth.
pub const POLL_DEGRADED_MS: i64 = 5_000;

/// How often the derived, clock-based row signals are re-evaluated.
pub const TICK_MS: i64 = 5_000;

pub enum SessionLiveAction {
    /// The periodic clock tick: redraws, and polls only when one is due.
    Tick,
    Ended {
        session_id: String,
        reason: SessionEndReason,
        detail: Option<String>,
    },
    MachineLiveness {
        machine_id: String,
        liveness: MachineLiveness,
    },
    /// Whether the socket is delivering; a dead socket shortens the poll.
    WsHealth(bool),
    /// Fold the selected row's subagent groups, or the group it sits in.
    ToggleFold,
    /// Fold the section the selected row belongs to.
    ToggleFoldSection,
    /// Fold everything, or — when nothing is open — unfold everything.
    ToggleFoldAll,
}

/// Requests made, sent and collapsed by the debounce. Surfaced in the status
/// bar so the drop in request count is observable rather than asserted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RefreshCounters {
    pub requested: u64,
    pub sent: u64,
    pub suppressed: u64,
}

pub fn reduce_session_live(app: &mut App, action: SessionLiveAction) -> Vec<Effect> {
    match action {
        SessionLiveAction::Tick => {
            let due = app.clock_ms - app.last_refresh_ms >= poll_period_ms(app);
            if due { refresh(app) } else { Vec::new() }
        }
        SessionLiveAction::Ended { session_id, reason, detail } => {
            apply_end(app, &session_id, reason, detail);
            Vec::new()
        }
        SessionLiveAction::MachineLiveness { machine_id, liveness } => {
            app.machine_liveness.insert(machine_id, liveness);
            Vec::new()
        }
        SessionLiveAction::WsHealth(healthy) => {
            app.ws_healthy = healthy;
            Vec::new()
        }
        SessionLiveAction::ToggleFold => toggle_fold(app),
        SessionLiveAction::ToggleFoldSection => toggle_section(app),
        SessionLiveAction::ToggleFoldAll => toggle_fold_all(app),
    }
}

fn toggle_fold(app: &mut App) -> Vec<Effect> {
    let Some(id) = app.selected_session_id() else { return Vec::new() };
    let scope = super::session_list::fold_scope(&app.list_rows(), &id);
    if scope.is_empty() {
        return Vec::new();
    }
    for (group, total) in &scope {
        app.ui.toggle_group(group, *total);
    }
    settle(app)
}

fn toggle_section(app: &mut App) -> Vec<Effect> {
    let Some(key) = app.selected_session().map(|s| super::session_list::group_of(s).key()) else {
        return Vec::new();
    };
    app.ui.toggle_section(key);
    settle(app)
}

fn toggle_fold_all(app: &mut App) -> Vec<Effect> {
    let probe = crate::config::uistate::UiState::probe();
    let (groups, sections) =
        super::session_list::fold_targets(&super::session_list::rows(&app.sessions, &probe));
    app.ui.fold_all(&groups, &sections);
    settle(app)
}

/// Folding shrinks the visible list, so the selection has to come back inside it
/// before anything reads it again.
fn settle(app: &mut App) -> Vec<Effect> {
    let len = app.flattened_sessions().len();
    app.selected_index = if len == 0 { 0 } else { app.selected_index.min(len - 1) };
    vec![Effect::SaveUiState(app.ui.clone())]
}

const fn poll_period_ms(app: &App) -> i64 {
    if app.ws_healthy { POLL_HEALTHY_MS } else { POLL_DEGRADED_MS }
}

/// A refresh request, collapsed when one already went out inside the debounce
/// window. The suppressed one is not retried: the next tick covers it.
pub fn refresh(app: &mut App) -> Vec<Effect> {
    app.refresh.requested += 1;
    if app.refresh.sent > 0 && app.clock_ms - app.last_refresh_ms < REFRESH_DEBOUNCE_MS {
        app.refresh.suppressed += 1;
        return Vec::new();
    }
    app.refresh.sent += 1;
    app.last_refresh_ms = app.clock_ms;
    vec![Effect::RefreshSessions]
}

/// An ended session moves to Done and stops looking live, without waiting for a
/// refresh to say so.
fn apply_end(app: &mut App, session_id: &str, reason: SessionEndReason, detail: Option<String>) {
    let Some(s) = app.sessions.iter_mut().find(|s| s.id == session_id) else { return };
    s.end_reason = Some(reason);
    s.end_detail = detail;
    s.ended_at = Some(chrono::DateTime::from_timestamp_millis(app.clock_ms).unwrap_or_default());
    s.bucket = cctui_proto::classifier::Bucket::Done;
    s.liveness = Liveness::Dead;
    s.attention = None;
    s.activity_detail = None;
    app.update_aggregates();
}

/// The machine dot for a row: the live tier the socket reported, else what the
/// refresh last said about the session itself.
#[must_use]
pub fn machine_dot(app: &App, machine_id: &str) -> Option<MachineLiveness> {
    app.machine_liveness.get(machine_id).copied()
}

#[must_use]
pub const fn machine_glyph(liveness: MachineLiveness) -> &'static str {
    match liveness {
        MachineLiveness::Online => "▪",
        MachineLiveness::Stale => "▫",
        MachineLiveness::Offline => "✗",
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::classifier::Bucket;
    use cctui_proto::models::{Liveness, MachineLiveness, SessionEndReason};

    use super::{POLL_DEGRADED_MS, POLL_HEALTHY_MS, REFRESH_DEBOUNCE_MS, SessionLiveAction};
    use crate::app::action::Effect;
    use crate::app::{Action, App, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "blocked"),
        ];
        app.update_aggregates();
        app
    }

    fn live(app: &mut App, action: SessionLiveAction) -> Vec<Effect> {
        reduce(app, Action::SessionLive(action))
    }

    fn refreshed(effects: &[Effect]) -> bool {
        effects.iter().any(|e| matches!(e, Effect::RefreshSessions))
    }

    #[test]
    fn ending_a_session_moves_it_to_done_without_a_refresh() {
        let mut app = app();
        app.clock_ms = 10_000;
        let effects = live(
            &mut app,
            SessionLiveAction::Ended {
                session_id: "s-b".to_owned(),
                reason: SessionEndReason::MachineOffline,
                detail: Some("daemon gone".to_owned()),
            },
        );
        assert!(effects.is_empty(), "the row is edited in place, not refetched");
        let s = &app.sessions[1];
        assert_eq!(s.bucket, Bucket::Done);
        assert_eq!(s.liveness, Liveness::Dead);
        assert_eq!(s.end_reason, Some(SessionEndReason::MachineOffline));
        assert_eq!(s.end_detail.as_deref(), Some("daemon gone"));
        assert!(s.attention.is_none());
        assert_eq!(
            crate::app::session_list::group_of(s),
            crate::app::session_list::Group::Bucket(Bucket::Done)
        );
    }

    #[test]
    fn an_end_for_an_unknown_session_is_ignored() {
        let mut app = app();
        assert!(
            live(
                &mut app,
                SessionLiveAction::Ended {
                    session_id: "nope".to_owned(),
                    reason: SessionEndReason::Killed,
                    detail: None,
                },
            )
            .is_empty()
        );
        assert!(app.sessions.iter().all(|s| s.end_reason.is_none()));
    }

    #[test]
    fn machine_liveness_updates_the_dot_and_nothing_else() {
        let mut app = app();
        live(
            &mut app,
            SessionLiveAction::MachineLiveness {
                machine_id: "orion".to_owned(),
                liveness: MachineLiveness::Stale,
            },
        );
        assert_eq!(super::machine_dot(&app, "orion"), Some(MachineLiveness::Stale));
        assert_eq!(super::machine_dot(&app, "elsewhere"), None);
        assert_eq!(super::machine_glyph(MachineLiveness::Offline), "✗");
    }

    #[test]
    fn refreshes_inside_the_debounce_window_collapse_into_one() {
        let mut app = app();
        app.clock_ms = 100_000;
        assert!(refreshed(&reduce(&mut app, Action::RefreshSessions)));
        assert!(!refreshed(&reduce(&mut app, Action::RefreshSessions)));
        assert!(!refreshed(&reduce(&mut app, Action::RefreshSessions)));
        app.clock_ms += REFRESH_DEBOUNCE_MS;
        assert!(refreshed(&reduce(&mut app, Action::RefreshSessions)));
        assert_eq!(app.refresh.requested, 4);
        assert_eq!(app.refresh.sent, 2);
        assert_eq!(app.refresh.suppressed, 2);
    }

    #[test]
    fn a_healthy_socket_stretches_the_poll_and_a_dead_one_shortens_it() {
        let mut app = app();
        app.clock_ms = 100_000;
        live(&mut app, SessionLiveAction::WsHealth(true));
        assert!(refreshed(&live(&mut app, SessionLiveAction::Tick)), "the first tick always polls");

        app.clock_ms += POLL_DEGRADED_MS;
        assert!(!refreshed(&live(&mut app, SessionLiveAction::Tick)));
        app.clock_ms += POLL_HEALTHY_MS - POLL_DEGRADED_MS;
        assert!(refreshed(&live(&mut app, SessionLiveAction::Tick)));

        live(&mut app, SessionLiveAction::WsHealth(false));
        app.clock_ms += POLL_DEGRADED_MS;
        assert!(refreshed(&live(&mut app, SessionLiveAction::Tick)));
    }

    fn with_subagents() -> App {
        let mut app = App::new();
        app.sessions = vec![crate::testsupport::session("s-p", "cctui", "active", "working")];
        for i in 0..12 {
            app.sessions.push(crate::testsupport::subagent(&format!("s-c{i:02}"), "s-p", "lane"));
        }
        app.sessions.push(crate::testsupport::pinned_session("s-pin", "p"));
        app.update_aggregates();
        app
    }

    fn visible(app: &App) -> Vec<String> {
        app.flattened_sessions().iter().map(|s| s.id.clone()).collect()
    }

    #[test]
    fn folding_a_parent_hides_its_subagents_and_persists_the_choice() {
        let mut app = with_subagents();
        assert_eq!(visible(&app), ["s-pin", "s-p"], "a big group starts folded");

        app.selected_index = 1;
        let effects = live(&mut app, SessionLiveAction::ToggleFold);
        assert!(matches!(effects.as_slice(), [Effect::SaveUiState(_)]));
        assert_eq!(visible(&app).len(), 14);

        live(&mut app, SessionLiveAction::ToggleFold);
        assert_eq!(visible(&app), ["s-pin", "s-p"]);
    }

    #[test]
    fn folding_from_a_child_folds_the_group_it_sits_in() {
        let mut app = with_subagents();
        app.selected_index = 1;
        live(&mut app, SessionLiveAction::ToggleFold);
        app.selected_index = 7;
        live(&mut app, SessionLiveAction::ToggleFold);
        assert_eq!(visible(&app), ["s-pin", "s-p"]);
        assert_eq!(app.selected_index, 1, "the selection comes back inside the list");
    }

    #[test]
    fn folding_a_section_keeps_the_other_sections() {
        let mut app = with_subagents();
        app.selected_index = 0;
        live(&mut app, SessionLiveAction::ToggleFoldSection);
        assert_eq!(visible(&app), ["s-p"]);
        assert_eq!(app.selected_index, 0);
        live(&mut app, SessionLiveAction::ToggleFoldSection);
        assert_eq!(visible(&app), ["s-pin", "s-p"]);
    }

    #[test]
    fn fold_all_closes_everything_then_opens_everything() {
        let mut app = with_subagents();
        live(&mut app, SessionLiveAction::ToggleFoldAll);
        assert!(visible(&app).is_empty(), "every section is folded");
        live(&mut app, SessionLiveAction::ToggleFoldAll);
        assert_eq!(visible(&app).len(), 14, "and every group opens with them");
    }

    #[test]
    fn folding_with_nothing_selected_changes_nothing() {
        let mut app = App::new();
        assert!(live(&mut app, SessionLiveAction::ToggleFold).is_empty());
        assert!(live(&mut app, SessionLiveAction::ToggleFoldSection).is_empty());
    }

    #[test]
    fn a_tick_alone_never_refetches_just_to_re_evaluate_the_clock() {
        let mut app = app();
        app.clock_ms = 100_000;
        app.ws_healthy = true;
        reduce(&mut app, Action::RefreshSessions);
        let before = app.refresh.sent;
        for _ in 0..4 {
            app.clock_ms += super::TICK_MS - 1;
            let _ = live(&mut app, SessionLiveAction::Tick);
        }
        assert!(app.refresh.sent > before, "the poll still fires eventually");
        assert!(app.refresh.sent - before <= 1, "one poll for four ticks");
    }
}
