//! Live session-list updates: websocket events edit rows in place, and the
//! REST refresh becomes a slow safety net rather than the source of truth.

use cctui_proto::models::{Liveness, MachineLiveness};

use super::action::Effect;
use super::state::App;

/// Two refreshes closer together than this collapse into one.
pub const REFRESH_DEBOUNCE_MS: i64 = 2_000;

/// Poll period while the socket is delivering events.
pub const POLL_HEALTHY_MS: i64 = 15_000;

/// Poll period with no live socket: the refresh is the only source of truth.
pub const POLL_DEGRADED_MS: i64 = 5_000;

/// The app clock's period. Set by the tightest consumer — delivery retry
/// ladders — not by the poll, which gates itself on its own elapsed period.
pub const TICK_MS: u64 = 500;

pub enum SessionLiveAction {
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
    let visible = super::list_view::visible_refs(&app.sessions, &app.list_shape);
    let (groups, sections) = super::session_list::fold_targets(&super::session_list::rows_by(
        &visible,
        &app.sessions,
        &probe,
        app.list_shape.group_by,
    ));
    let sections: Vec<&str> = sections.iter().map(String::as_str).collect();
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

/// The half of ending a session that moves its row: `bucket` drives the group,
/// so without this the row stays under Working until the next poll.
///
/// Called from `attention::end_session`, which owns everything else about an end
/// (status, badge, toast, dropping the session's permission cards).
pub fn mark_row_ended(s: &mut cctui_proto::api::SessionListItem) {
    s.bucket = cctui_proto::classifier::Bucket::Done;
    s.liveness = Liveness::Dead;
    s.attention = None;
    s.activity_detail = None;
}

/// Polls the session list when one is due, given how healthy the socket is.
pub fn poll_if_due(app: &mut App) -> Vec<Effect> {
    if app.clock_ms - app.last_refresh_ms >= poll_period_ms(app) {
        return refresh(app);
    }
    Vec::new()
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
    use cctui_proto::models::{Liveness, MachineLiveness};

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

    /// The shared 500ms clock tick, which also drives the poll.
    fn tick(app: &mut App) -> Vec<Effect> {
        reduce(app, Action::Tick)
    }

    fn refreshed(effects: &[Effect]) -> bool {
        effects.iter().any(|e| matches!(e, Effect::RefreshSessions))
    }

    #[test]
    fn ending_a_session_moves_its_row_to_done_without_a_refresh() {
        let mut app = app();
        app.clock_ms = 10_000;
        let effects = reduce(
            &mut app,
            Action::Attention(crate::app::attention::AttentionAction::SessionEnded {
                session_id: "s-b".to_owned(),
                reason: cctui_proto::models::SessionEndReason::MachineOffline,
                detail: Some("daemon gone".to_owned()),
            }),
        );
        assert!(effects.is_empty(), "the row is edited in place, not refetched");
        let s = &app.sessions[1];
        assert_eq!(s.bucket, Bucket::Done);
        assert_eq!(s.liveness, Liveness::Dead);
        assert!(s.attention.is_none());
        assert_eq!(
            crate::app::session_list::group_of(s),
            crate::app::session_list::Group::Bucket(Bucket::Done),
            "the group follows the bucket, so the row moves immediately"
        );
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
        assert!(refreshed(&tick(&mut app)), "the first tick always polls");

        app.clock_ms += POLL_DEGRADED_MS;
        assert!(!refreshed(&tick(&mut app)));
        app.clock_ms += POLL_HEALTHY_MS - POLL_DEGRADED_MS;
        assert!(refreshed(&tick(&mut app)));

        live(&mut app, SessionLiveAction::WsHealth(false));
        app.clock_ms += POLL_DEGRADED_MS;
        assert!(refreshed(&tick(&mut app)));
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

    /// `S` always acts on the selected row's section, and folding one moves the
    /// selection into the next open section — so repeated `S` folds each section
    /// in turn rather than toggling one. `Z` is the way back out.
    #[test]
    fn folding_a_section_keeps_the_other_sections_and_walks_on() {
        let mut app = with_subagents();
        app.selected_index = 0;
        live(&mut app, SessionLiveAction::ToggleFoldSection);
        assert_eq!(visible(&app), ["s-p"], "only Pinned is folded");
        assert_eq!(app.selected_index, 0);

        live(&mut app, SessionLiveAction::ToggleFoldSection);
        assert!(visible(&app).is_empty(), "the selection had moved to Working, so S folded it");

        live(&mut app, SessionLiveAction::ToggleFoldAll);
        assert_eq!(visible(&app).len(), 14, "Z is the escape from a fully folded list");
    }

    #[test]
    fn unfolding_a_section_restores_only_that_section() {
        let mut app = with_subagents();
        app.selected_index = 1;
        live(&mut app, SessionLiveAction::ToggleFoldSection);
        assert_eq!(visible(&app), ["s-pin"], "Working folded, Pinned untouched");
        app.ui.toggle_section("working");
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
        for _ in 0..40 {
            app.clock_ms += 500;
            let _ = tick(&mut app);
        }
        assert!(app.refresh.sent > before, "the poll still fires eventually");
        assert!(app.refresh.sent - before <= 2, "40 ticks over 20s poll at most twice");
    }
}
