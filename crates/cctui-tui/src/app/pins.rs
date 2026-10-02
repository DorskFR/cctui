//! Per-user message pins and the jump they address.

use std::collections::{BTreeSet, HashMap};

use super::action::Effect;
use super::conversation_store::PageKind;
use super::state::{App, LineKind, View};
use super::toast::Level;

/// Longest excerpt a pin row shows, as the web UI's panel caps it.
const EXCERPT_MAX: usize = 90;

/// How many older pages a jump may pull in before it gives up.
const MAX_JUMP_STEPS: u32 = 40;

/// One row of the pins list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PinRow {
    pub seq: i64,
    pub role: &'static str,
    pub excerpt: String,
}

/// The open pins list.
#[derive(Debug, Default)]
pub struct PinsList {
    pub rows: Vec<PinRow>,
    pub selected: usize,
}

/// A jump whose target is not in the fetched window yet.
#[derive(Debug, Clone, Copy)]
struct PendingJump {
    seq: i64,
    steps: u32,
}

#[derive(Debug, Default)]
pub struct PinState {
    by_session: HashMap<String, BTreeSet<i64>>,
    /// Bumped on every change, so the transcript's render cache rebuilds.
    pub epoch: u64,
    pub list: Option<PinsList>,
    pending_jump: Option<PendingJump>,
}

impl PinState {
    pub fn pinned(&self, session_id: &str, seq: i64) -> bool {
        self.by_session.get(session_id).is_some_and(|seqs| seqs.contains(&seq))
    }

    pub fn seqs(&self, session_id: &str) -> impl Iterator<Item = i64> + '_ {
        self.by_session.get(session_id).into_iter().flatten().copied()
    }

    pub fn count(&self, session_id: &str) -> usize {
        self.by_session.get(session_id).map_or(0, BTreeSet::len)
    }

    fn replace(&mut self, session_id: String, seqs: BTreeSet<i64>) {
        if self.by_session.get(&session_id) == Some(&seqs) {
            return;
        }
        self.by_session.insert(session_id, seqs);
        self.epoch = self.epoch.wrapping_add(1);
    }

    fn set(&mut self, session_id: &str, seq: i64, pinned: bool) {
        let seqs = self.by_session.entry(session_id.to_owned()).or_default();
        let changed = if pinned { seqs.insert(seq) } else { seqs.remove(&seq) };
        if changed {
            self.epoch = self.epoch.wrapping_add(1);
        }
    }
}

pub enum PinAction {
    /// `GET /sessions/{id}/pins` answered.
    Loaded {
        session_id: String,
        seqs: Vec<i64>,
    },
    /// The server accepted a pin or an unpin.
    Changed {
        session_id: String,
        seq: i64,
        pinned: bool,
    },
    /// `m`: pin or unpin the focused transcript line.
    Toggle,
    OpenList,
    CloseList,
    SelectNext,
    SelectPrev,
    Jump,
    UnpinSelected,
}

pub fn reduce_pins(app: &mut App, action: PinAction) -> Vec<Effect> {
    match action {
        PinAction::Loaded { session_id, seqs } => {
            app.pins.replace(session_id, seqs.into_iter().collect());
            Vec::new()
        }
        PinAction::Changed { session_id, seq, pinned } => {
            app.pins.set(&session_id, seq, pinned);
            refresh_list(app);
            Vec::new()
        }
        PinAction::Toggle => toggle(app),
        PinAction::OpenList => open_list(app),
        PinAction::CloseList => {
            close_list(app);
            Vec::new()
        }
        PinAction::SelectNext => {
            move_selection(app, 1);
            Vec::new()
        }
        PinAction::SelectPrev => {
            move_selection(app, -1);
            Vec::new()
        }
        PinAction::Jump => jump_to_selected(app),
        PinAction::UnpinSelected => unpin_selected(app),
    }
}

/// Read a session's pins as its conversation opens: the web UI may have
/// changed them since this TUI last looked.
pub fn on_open(session_id: &str) -> Effect {
    Effect::LoadPins { session_id: session_id.to_owned() }
}

/// The seq of the line the cursor is on, when that line has one. A line still
/// in flight has no server address, so it cannot be pinned.
fn focused_seq(app: &mut App) -> Option<i64> {
    let cursor = app.line_cursor?;
    let session_id = app.selected_session_id()?;
    let entry = app.conversation_mut(&session_id).entries().get(cursor)?;
    entry.sequenced.then_some(entry.seq)
}

fn toggle(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    if app.line_cursor.is_none() {
        app.toast(Level::Info, "press v to select a line first");
        return Vec::new();
    }
    let Some(seq) = focused_seq(app) else {
        app.toast(Level::Warn, "this line has no server address yet");
        return Vec::new();
    };
    if app.pins.pinned(&session_id, seq) {
        vec![Effect::UnpinMessage { session_id, seq }]
    } else {
        vec![Effect::PinMessage { session_id, seq }]
    }
}

fn rows_for(app: &mut App, session_id: &str) -> Vec<PinRow> {
    let loaded: HashMap<i64, (&'static str, String)> = app
        .conversation_mut(session_id)
        .entries()
        .iter()
        .filter(|entry| entry.sequenced)
        .map(|entry| (entry.seq, (role_of(entry.line.kind), excerpt(&entry.line.text))))
        .collect();
    app.pins
        .seqs(session_id)
        .map(|seq| match loaded.get(&seq) {
            Some((role, excerpt)) => PinRow { seq, role, excerpt: excerpt.clone() },
            None => PinRow { seq, role: "—", excerpt: "(not loaded)".to_owned() },
        })
        .collect()
}

/// A pin can outlive the window the TUI has fetched, so a row renders for a
/// seq whose line is not loaded.
#[must_use]
pub fn excerpt(text: &str) -> String {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return "(no text)".to_owned();
    }
    if collapsed.chars().count() <= EXCERPT_MAX {
        return collapsed;
    }
    let head: String = collapsed.chars().take(EXCERPT_MAX - 1).collect();
    format!("{head}…")
}

#[must_use]
pub const fn role_of(kind: LineKind) -> &'static str {
    match kind {
        LineKind::User => "user",
        LineKind::Assistant | LineKind::Image => "assistant",
        LineKind::Thinking { .. } => "thinking",
        LineKind::Tool { .. } | LineKind::Result { .. } => "tool",
        LineKind::Peer => "peer",
        LineKind::System | LineKind::Marker | LineKind::Reply => "system",
        LineKind::Reset | LineKind::Compact | LineKind::Summary => "boundary",
    }
}

fn open_list(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let rows = rows_for(app, &session_id);
    if rows.is_empty() {
        app.toast(Level::Info, "no pinned messages — press m on a line to pin it");
        return Vec::new();
    }
    app.pins.list = Some(PinsList { rows, selected: 0 });
    app.router.push(View::Pins);
    Vec::new()
}

fn close_list(app: &mut App) {
    app.pins.list = None;
    if app.view() == View::Pins {
        app.router.pop();
    }
}

/// Keep the open list in step with a pin that changed under it.
fn refresh_list(app: &mut App) {
    if app.pins.list.is_none() {
        return;
    }
    let Some(session_id) = app.selected_session_id() else { return };
    let rows = rows_for(app, &session_id);
    if rows.is_empty() {
        close_list(app);
        return;
    }
    if let Some(list) = app.pins.list.as_mut() {
        list.selected = list.selected.min(rows.len() - 1);
        list.rows = rows;
    }
}

fn move_selection(app: &mut App, delta: i32) {
    let Some(list) = app.pins.list.as_mut() else { return };
    if list.rows.is_empty() {
        return;
    }
    let end = list.rows.len() - 1;
    list.selected = if delta < 0 {
        list.selected.checked_sub(1).unwrap_or(end)
    } else if list.selected >= end {
        0
    } else {
        list.selected + 1
    };
}

fn selected_seq(app: &App) -> Option<i64> {
    let list = app.pins.list.as_ref()?;
    list.rows.get(list.selected).map(|row| row.seq)
}

fn unpin_selected(app: &App) -> Vec<Effect> {
    let Some(seq) = selected_seq(app) else { return Vec::new() };
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    vec![Effect::UnpinMessage { session_id, seq }]
}

fn jump_to_selected(app: &mut App) -> Vec<Effect> {
    let Some(seq) = selected_seq(app) else { return Vec::new() };
    close_list(app);
    jump_to_seq(app, seq)
}

/// Put the line cursor on `seq`. The transcript is windowed, so a seq older
/// than the fetched window is chased by paging backwards a bounded number of
/// times, as the web UI's jump primitive does.
pub fn jump_to_seq(app: &mut App, seq: i64) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    if let Some(index) = index_of(app, &session_id, seq) {
        app.pins.pending_jump = None;
        app.line_cursor = Some(index);
        app.follow_tail = false;
        return Vec::new();
    }
    let steps = app.pins.pending_jump.map_or(0, |j| j.steps);
    if steps >= MAX_JUMP_STEPS {
        app.pins.pending_jump = None;
        app.toast(Level::Warn, "gave up looking for that message");
        return Vec::new();
    }
    let Some((page, claim)) = app.conversation_mut(&session_id).begin_older() else {
        app.pins.pending_jump = None;
        app.toast(Level::Warn, "that message is no longer in the transcript");
        return Vec::new();
    };
    app.pins.pending_jump = Some(PendingJump { seq, steps: steps + 1 });
    vec![Effect::LoadConversationPage {
        session_id,
        kind: PageKind::Older,
        claim: Some(claim),
        page,
        etag: None,
    }]
}

/// An older page landed: resume a jump that was waiting for it.
pub fn after_page(app: &mut App, session_id: &str) -> Vec<Effect> {
    let Some(jump) = app.pins.pending_jump else { return Vec::new() };
    if app.selected_session_id().as_deref() != Some(session_id) {
        return Vec::new();
    }
    jump_to_seq(app, jump.seq)
}

/// A page fetch failed: a jump waiting on it would spin.
pub fn page_failed(app: &mut App) {
    if app.pins.pending_jump.take().is_some() {
        app.toast(Level::Warn, "could not load more history for that jump");
    }
}

fn index_of(app: &mut App, session_id: &str, seq: i64) -> Option<usize> {
    app.conversation_mut(session_id)
        .entries()
        .iter()
        .position(|entry| entry.sequenced && entry.seq == seq)
}

#[cfg(test)]
pub fn store_len(app: &mut App, session_id: &str) -> usize {
    app.conversation_mut(session_id).entries().len()
}

#[cfg(test)]
mod tests {
    use super::{PinAction, excerpt, store_len};
    use crate::app::action::Effect;
    use crate::app::conversation::ConversationAction;
    use crate::app::conversation_store::PageKind;
    use crate::app::state::{App, ConversationLine, LineKind, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        reduce(&mut app, Action::OpenSelectedConversation);
        app
    }

    fn page(app: &mut App, kind: PageKind, rows: &[(i64, &str)], has_more: bool) {
        let rows = rows
            .iter()
            .map(|(seq, text)| (*seq, ConversationLine::new(LineKind::Assistant, *text, 0)))
            .collect();
        // An older reply carries the claim of the request it answers, the way
        // the effect does.
        let claim = (kind == PageKind::Older)
            .then(|| app.conversation_mut("s-a").outstanding_claim())
            .flatten();
        reduce(
            app,
            Action::Conversation(ConversationAction::Loaded {
                session_id: "s-a".to_owned(),
                kind,
                claim,
                rows,
                etag: None,
                has_more,
            }),
        );
    }

    fn pins(app: &mut App, seqs: &[i64]) {
        reduce(
            app,
            Action::Pins(PinAction::Loaded { session_id: "s-a".to_owned(), seqs: seqs.to_vec() }),
        );
    }

    fn line_select(app: &mut App) {
        reduce(app, Action::Conversation(ConversationAction::ToggleLineCursor));
    }

    #[test]
    fn m_pins_the_focused_line_and_unpins_it_again() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(7, "first"), (9, "second")], false);
        line_select(&mut app);
        assert_eq!(app.line_cursor, Some(1));

        let effects = reduce(&mut app, Action::Pins(PinAction::Toggle));
        match effects.as_slice() {
            [Effect::PinMessage { session_id, seq }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(*seq, 9, "the cursor's own line is pinned");
            }
            _ => panic!("expected a pin request"),
        }

        reduce(
            &mut app,
            Action::Pins(PinAction::Changed { session_id: "s-a".to_owned(), seq: 9, pinned: true }),
        );
        assert!(app.pins.pinned("s-a", 9));
        assert_eq!(app.pins.count("s-a"), 1);

        let effects = reduce(&mut app, Action::Pins(PinAction::Toggle));
        assert!(matches!(effects.as_slice(), [Effect::UnpinMessage { seq: 9, .. }]));
    }

    #[test]
    fn pinning_needs_a_focused_line_with_a_server_address() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(7, "first")], false);
        assert!(reduce(&mut app, Action::Pins(PinAction::Toggle)).is_empty());
        assert!(app.toasts.latest().is_some(), "the user is told to select a line");

        crate::app::conversation::stream(
            &mut app,
            "s-a",
            None,
            ConversationLine::new(LineKind::Assistant, "live, unsequenced", 0),
        );
        line_select(&mut app);
        assert!(reduce(&mut app, Action::Pins(PinAction::Toggle)).is_empty());
        assert_eq!(app.pins.count("s-a"), 0);
    }

    #[test]
    fn a_pin_change_bumps_the_epoch_so_the_transcript_redraws() {
        let mut app = app();
        let before = app.pins.epoch;
        pins(&mut app, &[3]);
        assert_ne!(app.pins.epoch, before);
        let after_load = app.pins.epoch;
        pins(&mut app, &[3]);
        assert_eq!(app.pins.epoch, after_load, "an identical list is not a change");
    }

    #[test]
    fn the_list_shows_a_row_per_pin_and_enter_jumps_to_it() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(4, "older one"), (5, "newer one")], false);
        pins(&mut app, &[4]);

        reduce(&mut app, Action::Pins(PinAction::OpenList));
        assert_eq!(app.view(), View::Pins);
        let list = app.pins.list.as_ref().expect("an open list");
        assert_eq!(list.rows.len(), 1);
        assert_eq!(list.rows[0].seq, 4);
        assert_eq!(list.rows[0].role, "assistant");
        assert_eq!(list.rows[0].excerpt, "older one");

        app.line_cursor = None;
        app.follow_tail = true;
        assert!(reduce(&mut app, Action::Pins(PinAction::Jump)).is_empty());
        assert_eq!(app.view(), View::Conversation, "jumping closes the list");
        assert_eq!(app.line_cursor, Some(0), "the cursor lands on the pinned line");
        assert!(!app.follow_tail, "a jump detaches from the tail");
    }

    #[test]
    fn an_empty_list_says_so_instead_of_opening() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(4, "a")], false);
        assert!(reduce(&mut app, Action::Pins(PinAction::OpenList)).is_empty());
        assert_eq!(app.view(), View::Conversation);
        assert!(app.pins.list.is_none());
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn the_selection_wraps_and_unpinning_from_the_list_asks_the_server() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(4, "a"), (5, "b"), (6, "c")], false);
        pins(&mut app, &[4, 6]);
        reduce(&mut app, Action::Pins(PinAction::OpenList));

        reduce(&mut app, Action::Pins(PinAction::SelectPrev));
        assert_eq!(app.pins.list.as_ref().expect("list").selected, 1);
        reduce(&mut app, Action::Pins(PinAction::SelectNext));
        assert_eq!(app.pins.list.as_ref().expect("list").selected, 0);

        let effects = reduce(&mut app, Action::Pins(PinAction::UnpinSelected));
        assert!(matches!(effects.as_slice(), [Effect::UnpinMessage { seq: 4, .. }]));

        reduce(
            &mut app,
            Action::Pins(PinAction::Changed {
                session_id: "s-a".to_owned(),
                seq: 4,
                pinned: false,
            }),
        );
        let list = app.pins.list.as_ref().expect("the list stays open");
        assert_eq!(list.rows.len(), 1, "the unpinned row is gone");
        assert_eq!(list.rows[0].seq, 6);
    }

    #[test]
    fn unpinning_the_last_pin_closes_the_list() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(4, "a")], false);
        pins(&mut app, &[4]);
        reduce(&mut app, Action::Pins(PinAction::OpenList));
        reduce(
            &mut app,
            Action::Pins(PinAction::Changed {
                session_id: "s-a".to_owned(),
                seq: 4,
                pinned: false,
            }),
        );
        assert!(app.pins.list.is_none());
        assert_eq!(app.view(), View::Conversation);
    }

    #[test]
    fn a_pin_outside_the_fetched_window_is_chased_by_paging_older() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(20, "newest")], true);
        pins(&mut app, &[11]);
        reduce(&mut app, Action::Pins(PinAction::OpenList));

        let effects = reduce(&mut app, Action::Pins(PinAction::Jump));
        match effects.as_slice() {
            [Effect::LoadConversationPage { kind: PageKind::Older, page, .. }] => {
                assert_eq!(page.before, Some(20));
            }
            _ => panic!("expected an older page fetch, got {} effects", effects.len()),
        }
        assert_eq!(app.line_cursor, None, "nothing to land on yet");

        page(&mut app, PageKind::Older, &[(11, "the pinned one"), (12, "next")], false);
        assert_eq!(store_len(&mut app, "s-a"), 3);
        assert_eq!(app.line_cursor, Some(0), "the landed page resumes the jump");
    }

    #[test]
    fn a_jump_that_runs_out_of_history_gives_up_with_a_toast() {
        let mut app = app();
        page(&mut app, PageKind::Latest, &[(20, "newest")], false);
        pins(&mut app, &[11]);
        reduce(&mut app, Action::Pins(PinAction::OpenList));
        assert!(reduce(&mut app, Action::Pins(PinAction::Jump)).is_empty());
        assert_eq!(app.line_cursor, None);
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn an_excerpt_collapses_whitespace_and_is_capped() {
        assert_eq!(excerpt("  two   lines\nhere "), "two lines here");
        assert_eq!(excerpt("   "), "(no text)");
        let long = "x".repeat(200);
        let cut = excerpt(&long);
        assert_eq!(cut.chars().count(), 90);
        assert!(cut.ends_with('…'));
    }

    /// R12: the pins overlay sits over the conversation, so the cursor may
    /// address a different row than the session being read.
    #[test]
    fn unpinning_from_the_overlay_targets_the_open_conversation_not_the_cursor() {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        reduce(&mut app, Action::OpenSelectedConversation);
        let opened = app.subscribed.clone().expect("a subscription");

        page(&mut app, PageKind::Latest, &[(4, "a")], false);
        pins(&mut app, &[4]);
        reduce(&mut app, Action::Pins(PinAction::OpenList));
        assert_eq!(app.view(), View::Pins, "the overlay is on top");

        app.selected_index = 1;

        let effects = reduce(&mut app, Action::Pins(PinAction::UnpinSelected));
        match effects.as_slice() {
            [Effect::UnpinMessage { session_id, seq: 4 }] => {
                assert_eq!(*session_id, opened);
            }
            _ => panic!("expected one unpin for the open conversation"),
        }
    }
}
