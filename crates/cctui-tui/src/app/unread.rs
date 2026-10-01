//! Unread message counts: what arrived in a session while you were not looking.
//!
//! The server's `unread_count` is authoritative and arrives with every list
//! refresh; these counters keep the badge honest between refreshes and zero it
//! the moment a conversation is opened, rather than a poll later.

use std::collections::HashMap;

use cctui_proto::api::SessionListItem;

use super::action::Effect;
use super::state::{App, LineKind, View};

/// How long an open conversation waits before telling the server it has been
/// read again. Short enough to feel immediate, long enough that a streaming
/// turn does not post once per line.
pub const SEEN_DEBOUNCE_MS: i64 = 1_500;

#[derive(Debug, Default)]
pub struct Unread {
    /// When each session was last reported seen, so the debounce has a floor.
    seen_at: HashMap<String, i64>,
}

impl Unread {
    fn due(&self, session_id: &str, now_ms: i64) -> bool {
        self.seen_at.get(session_id).is_none_or(|at| now_ms.saturating_sub(*at) >= SEEN_DEBOUNCE_MS)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnreadAction {
    ToggleOnly,
}

pub fn reduce_unread(app: &mut App, action: UnreadAction) -> Vec<Effect> {
    match action {
        UnreadAction::ToggleOnly => {
            app.ui.unread_only = !app.ui.unread_only;
            if app.ui.unread_only && total(app) == 0 {
                app.toast(super::toast::Level::Info, "nothing unread");
            }
            clamp_selection(app);
            vec![Effect::SaveUiState(app.ui.clone())]
        }
    }
}

/// Hiding rows can leave the cursor past the end of the list.
fn clamp_selection(app: &mut App) {
    let len = app.flattened_sessions().len();
    app.selected_index = app.selected_index.min(len.saturating_sub(1));
}

/// A line arrived over the socket: count it unless the reader is looking at
/// that very session.
pub fn stream(app: &mut App, session_id: &str, kind: LineKind) {
    if !counts_as_unread(kind) || is_being_read(app, session_id) {
        return;
    }
    if let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id) {
        session.unread_count = session.unread_count.saturating_add(1).min(UNREAD_CAP);
    }
}

/// The server caps what it reports; the local bump matches so a long run in the
/// background cannot render a wider badge than a refresh would.
pub const UNREAD_CAP: u32 = 99;

/// Only what a person would call a message: the agent speaking, or another
/// session relaying something. Tool traffic, markers and footers are noise to
/// count.
const fn counts_as_unread(kind: LineKind) -> bool {
    matches!(kind, LineKind::Assistant | LineKind::Peer)
}

/// Whether this session's transcript is the one on screen. A conversation
/// behind an overlay still counts as being read: the overlay is transient and
/// the transcript is right there underneath.
fn is_being_read(app: &App, session_id: &str) -> bool {
    app.subscribed.as_deref() == Some(session_id)
        && app.selected_session_id().as_deref() == Some(session_id)
        && app.view() != View::SessionList
}

/// Opening a conversation clears its badge at once; the `POST .../seen` that
/// goes with it is [`Effect::MarkSeen`], which the caller already emits.
pub fn opened(app: &mut App, session_id: &str) {
    clear(app, session_id);
    app.unread.seen_at.insert(session_id.to_owned(), app.clock_ms);
}

fn clear(app: &mut App, session_id: &str) {
    if let Some(session) = app.sessions.iter_mut().find(|s| s.id == session_id) {
        session.unread_count = 0;
    }
}

/// Called on every tick: an open conversation keeps telling the server it is
/// read, so messages that land while it is on screen do not pile up a badge.
pub fn tick(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.subscribed.clone() else { return Vec::new() };
    if !is_being_read(app, &session_id) {
        return Vec::new();
    }
    let unread =
        app.sessions.iter().find(|s| s.id == session_id).is_some_and(|s| s.unread_count > 0);
    if !unread || !app.unread.due(&session_id, app.clock_ms) {
        return Vec::new();
    }
    clear(app, &session_id);
    app.unread.seen_at.insert(session_id.clone(), app.clock_ms);
    vec![Effect::MarkSeen { session_id }]
}

/// Every unread message across the list, for the header.
#[must_use]
pub fn total(app: &App) -> u32 {
    app.sessions.iter().map(|s| s.unread_count).sum()
}

/// Whether a row survives the unread-only filter: itself unread, or holding a
/// child that is. A parent is kept so the child it hides under stays reachable.
#[must_use]
pub fn keeps_row(sessions: &[SessionListItem], s: &SessionListItem) -> bool {
    if s.unread_count > 0 {
        return true;
    }
    sessions.iter().any(|k| k.unread_count > 0 && k.parent_id.as_deref() == Some(s.id.as_str()))
}

#[cfg(test)]
mod tests {
    use super::{SEEN_DEBOUNCE_MS, UNREAD_CAP, UnreadAction, keeps_row, total};
    use crate::app::action::Effect;
    use crate::app::state::{ConversationLine, LineKind};
    use crate::app::{Action, App, reduce};
    use crate::testsupport::{session, subagent};

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn stream(app: &mut App, session_id: &str, kind: LineKind) {
        reduce(
            app,
            Action::StreamLine {
                session_id: session_id.to_owned(),
                seq: None,
                line: Some(Box::new(ConversationLine::new(kind, "hello", 0))),
                usage: None,
            },
        );
    }

    fn unread_of(app: &App, id: &str) -> u32 {
        app.sessions.iter().find(|s| s.id == id).expect("a session").unread_count
    }

    #[test]
    fn a_message_for_a_session_you_are_not_reading_counts() {
        let mut app = app();
        stream(&mut app, "s-b", LineKind::Assistant);
        stream(&mut app, "s-b", LineKind::Peer);
        assert_eq!(unread_of(&app, "s-b"), 2);
        assert_eq!(total(&app), 2);
    }

    #[test]
    fn tool_traffic_and_bookkeeping_are_not_messages() {
        let mut app = app();
        for kind in [
            LineKind::Tool { category: crate::app::ToolCategory::Read },
            LineKind::Result { error: false },
            LineKind::Marker,
            LineKind::Summary,
            LineKind::System,
            LineKind::Reset,
            LineKind::User,
        ] {
            stream(&mut app, "s-b", kind);
        }
        assert_eq!(unread_of(&app, "s-b"), 0);
    }

    #[test]
    fn the_session_on_screen_never_accrues_a_badge() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        stream(&mut app, "s-a", LineKind::Assistant);
        assert_eq!(unread_of(&app, "s-a"), 0);

        // Still the open session, but the reader went back to the list.
        reduce(&mut app, Action::LeaveConversation);
        stream(&mut app, "s-a", LineKind::Assistant);
        assert_eq!(unread_of(&app, "s-a"), 1);
    }

    #[test]
    fn opening_a_conversation_zeroes_the_badge_at_once() {
        let mut app = app();
        stream(&mut app, "s-a", LineKind::Assistant);
        assert_eq!(unread_of(&app, "s-a"), 1);
        let effects = reduce(&mut app, Action::OpenSelectedConversation);
        assert_eq!(unread_of(&app, "s-a"), 0, "no waiting for the next poll");
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkSeen { .. })));
    }

    #[test]
    fn the_count_cannot_grow_past_what_a_refresh_would_report() {
        let mut app = app();
        for _ in 0..UNREAD_CAP + 20 {
            stream(&mut app, "s-b", LineKind::Assistant);
        }
        assert_eq!(unread_of(&app, "s-b"), UNREAD_CAP);
    }

    /// While a conversation is open the tick re-reports it, but not once per
    /// line of a streaming turn.
    #[test]
    fn the_debounce_marks_seen_at_most_once_a_window() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        app.clock_ms = 10_000;

        // A line that slipped in before the open call landed.
        app.sessions[0].unread_count = 3;
        let effects = reduce(&mut app, Action::Tick);
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkSeen { .. })));
        assert_eq!(unread_of(&app, "s-a"), 0);

        app.sessions[0].unread_count = 1;
        let effects = reduce(&mut app, Action::Tick);
        assert!(
            !effects.iter().any(|e| matches!(e, Effect::MarkSeen { .. })),
            "inside the window, nothing is posted"
        );

        app.clock_ms += SEEN_DEBOUNCE_MS;
        let effects = reduce(&mut app, Action::Tick);
        assert!(effects.iter().any(|e| matches!(e, Effect::MarkSeen { .. })));
    }

    #[test]
    fn a_tick_with_nothing_unread_posts_nothing() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        app.clock_ms = 10_000;
        assert!(
            !reduce(&mut app, Action::Tick).iter().any(|e| matches!(e, Effect::MarkSeen { .. }))
        );
    }

    #[test]
    fn the_tick_leaves_a_closed_conversation_alone() {
        let mut app = app();
        app.sessions[0].unread_count = 2;
        app.clock_ms = 10_000;
        assert!(
            !reduce(&mut app, Action::Tick).iter().any(|e| matches!(e, Effect::MarkSeen { .. }))
        );
        assert_eq!(unread_of(&app, "s-a"), 2);
    }

    #[test]
    fn unread_only_hides_the_rows_with_nothing_new() {
        let mut app = app();
        app.sessions[1].unread_count = 2;
        assert_eq!(app.flattened_sessions().len(), 2);

        let effects = reduce(&mut app, Action::Unread(UnreadAction::ToggleOnly));
        assert!(app.ui.unread_only);
        assert!(matches!(effects.as_slice(), [Effect::SaveUiState(_)]));
        let shown: Vec<&str> = app.flattened_sessions().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(shown, vec!["s-b"]);

        reduce(&mut app, Action::Unread(UnreadAction::ToggleOnly));
        assert_eq!(app.flattened_sessions().len(), 2);
    }

    /// Hiding the row the cursor was on must not leave it pointing off the end.
    #[test]
    fn the_cursor_survives_the_filter() {
        let mut app = app();
        app.sessions[0].unread_count = 1;
        reduce(&mut app, Action::SelectLast);
        assert_eq!(app.selected_index, 1);
        reduce(&mut app, Action::Unread(UnreadAction::ToggleOnly));
        assert_eq!(app.selected_index, 0);
        assert_eq!(app.selected_session_id().as_deref(), Some("s-a"));
    }

    #[test]
    fn toggling_onto_an_empty_list_says_so() {
        let mut app = app();
        reduce(&mut app, Action::Unread(UnreadAction::ToggleOnly));
        assert!(app.flattened_sessions().is_empty());
        assert_eq!(app.toasts.latest().expect("a toast").text, "nothing unread");
    }

    /// A parent with nothing new still shows while it holds an unread child.
    #[test]
    fn a_parent_is_kept_for_an_unread_child() {
        let mut app = app();
        app.sessions.push(subagent("s-kid", "s-a", "worker"));
        app.sessions.last_mut().expect("the kid").unread_count = 1;
        assert!(keeps_row(&app.sessions, &app.sessions[0]));
        assert!(!keeps_row(&app.sessions, &app.sessions[1]));

        reduce(&mut app, Action::Unread(UnreadAction::ToggleOnly));
        let shown: Vec<&str> = app.flattened_sessions().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(shown, vec!["s-a", "s-kid"]);
    }
}
