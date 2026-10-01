//! Row actions on the session list: pin, rename, archive, kill, and the
//! multi-select the batch archive runs over.
//!
//! Selection is held by session id, never by row index: a websocket event
//! reorders the list under the cursor, and an index-keyed selection would then
//! archive whatever slid into that slot.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent};

use super::action::Effect;
use super::session_list::{Group, Row};
use super::state::App;
use super::toast::Level;

/// How long the undo offer stands, matching the toast it rides on.
pub const UNDO_MS: i64 = super::toast::Toasts::TTL_MS;

/// What a `y` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Kill(String),
    /// Archive these ids; the label is what the undo toast calls them.
    Archive {
        ids: Vec<String>,
        label: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Confirm {
    pub prompt: String,
    pub action: Pending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rename {
    pub session_id: String,
    pub input: String,
}

/// An archive that can still be taken back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Undo {
    pub ids: Vec<String>,
    pub expires_ms: i64,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RowActionState {
    /// Ids the user has picked. Non-empty implies select mode is on.
    pub selected: HashSet<String>,
    /// True once `space` has been pressed, so the strip stays up after the
    /// last row is toggled back off.
    pub mode: bool,
    /// Where `V` measures its range from.
    pub anchor: Option<String>,
    pub confirm: Option<Confirm>,
    pub rename: Option<Rename>,
    pub undo: Option<Undo>,
}

impl RowActionState {
    #[must_use]
    pub fn is_selected(&self, session_id: &str) -> bool {
        self.selected.contains(session_id)
    }

    /// The strip's own text, or `None` when the list is in its normal state.
    #[must_use]
    pub fn strip(&self) -> Option<String> {
        if let Some(confirm) = &self.confirm {
            return Some(format!(" {} y/N", confirm.prompt));
        }
        if let Some(rename) = &self.rename {
            return Some(format!(" rename: {}_", rename.input));
        }
        if !self.mode {
            return None;
        }
        Some(format!(
            " -- SELECT ({}) --  space toggle  V range  * all  x archive  esc cancel",
            self.selected.len()
        ))
    }

    fn leave_select(&mut self) {
        self.selected.clear();
        self.anchor = None;
        self.mode = false;
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RowAction {
    TogglePin,
    RenameStart,
    RenameKey(KeyEvent),
    RenameCommit,
    RenameCancel,
    /// `x`: the selection if there is one, else the row under the cursor.
    ArchiveOrUnarchive,
    KillStart,
    /// `A`: everything in the section the cursor sits in.
    ArchiveSection,
    ConfirmYes,
    ConfirmNo,
    ToggleSelect,
    RangeToAnchor,
    SelectAllVisible,
    ClearSelection,
    Undo,
}

#[allow(clippy::too_many_lines)]
pub fn reduce_row_actions(app: &mut App, action: RowAction) -> Vec<Effect> {
    match action {
        RowAction::TogglePin => toggle_pin(app),

        RowAction::RenameStart => {
            let Some(session) = app.selected_session() else { return Vec::new() };
            let (session_id, input) =
                (session.id.clone(), session.name.clone().unwrap_or_default());
            app.row_actions.confirm = None;
            app.row_actions.rename = Some(Rename { session_id, input });
            Vec::new()
        }
        RowAction::RenameKey(key) => {
            if let Some(rename) = app.row_actions.rename.as_mut() {
                type_into(&mut rename.input, key);
            }
            Vec::new()
        }
        RowAction::RenameCancel => {
            app.row_actions.rename = None;
            Vec::new()
        }
        RowAction::RenameCommit => {
            let Some(rename) = app.row_actions.rename.take() else { return Vec::new() };
            let name = rename.input.trim().to_owned();
            if name.is_empty() {
                app.toast(Level::Warn, "a name must not be empty");
                return Vec::new();
            }
            vec![Effect::RenameSession { session_id: rename.session_id, name }]
        }

        RowAction::ArchiveOrUnarchive => archive_or_unarchive(app),
        RowAction::KillStart => {
            let Some(session_id) = app.selected_session_id() else { return Vec::new() };
            let prompt = format!("Kill {}?", label_of(app, &session_id));
            app.row_actions.rename = None;
            app.row_actions.confirm = Some(Confirm { prompt, action: Pending::Kill(session_id) });
            Vec::new()
        }
        RowAction::ArchiveSection => archive_section(app),

        RowAction::ConfirmYes => {
            let Some(confirm) = app.row_actions.confirm.take() else { return Vec::new() };
            match confirm.action {
                Pending::Kill(session_id) => vec![Effect::KillSession { session_id }],
                Pending::Archive { ids, label } => archive(app, ids, &label),
            }
        }
        RowAction::ConfirmNo => {
            app.row_actions.confirm = None;
            Vec::new()
        }

        RowAction::ToggleSelect => {
            let Some(id) = app.selected_session_id() else { return Vec::new() };
            app.row_actions.mode = true;
            if !app.row_actions.selected.remove(&id) {
                app.row_actions.selected.insert(id.clone());
            }
            app.row_actions.anchor = Some(id);
            Vec::new()
        }
        RowAction::RangeToAnchor => {
            range_to_anchor(app);
            Vec::new()
        }
        RowAction::SelectAllVisible => {
            let ids = visible_ids(app);
            if ids.is_empty() {
                return Vec::new();
            }
            app.row_actions.mode = true;
            app.row_actions.selected.extend(ids);
            Vec::new()
        }
        RowAction::ClearSelection => {
            if app.row_actions.confirm.take().is_some() || app.row_actions.rename.take().is_some() {
                return Vec::new();
            }
            app.row_actions.leave_select();
            Vec::new()
        }

        RowAction::Undo => undo(app),
    }
}

fn toggle_pin(app: &mut App) -> Vec<Effect> {
    let Some(session) = app.selected_session() else { return Vec::new() };
    let pinned = !session.pinned;
    let ids = vec![session.id.clone()];
    let what = label_of(app, &ids[0]);
    app.toast(Level::Info, format!("{} {what}", if pinned { "pinned" } else { "unpinned" }));
    vec![Effect::PinSessions { ids, pinned }]
}

/// `x`: a selection is a batch and asks first; one row acts at once, since the
/// undo toast is the confirmation.
fn archive_or_unarchive(app: &mut App) -> Vec<Effect> {
    if app.row_actions.mode && !app.row_actions.selected.is_empty() {
        let ids = selected_in_view_order(app);
        let label = format!("{} sessions", ids.len());
        app.row_actions.confirm = Some(Confirm {
            prompt: format!("Archive {} sessions?", ids.len()),
            action: Pending::Archive { ids, label },
        });
        return Vec::new();
    }
    let Some(session) = app.selected_session() else { return Vec::new() };
    let id = session.id.clone();
    let archived = session.status == cctui_proto::models::SessionStatus::Archived;
    let label = label_of(app, &id);
    if archived {
        app.toast(Level::Info, format!("unarchived {label}"));
        return vec![Effect::ArchiveSessions { ids: vec![id], archived: false }];
    }
    archive(app, vec![id], &label)
}

/// `A`: the section header the cursor is under, which is how "archive all
/// dispatched" is done.
fn archive_section(app: &mut App) -> Vec<Effect> {
    let Some(group) = section_of_selection(app) else { return Vec::new() };
    let ids = section_ids(app, group);
    if ids.is_empty() {
        return Vec::new();
    }
    let label = format!("{} in {}", ids.len(), group.label());
    app.row_actions.rename = None;
    app.row_actions.confirm = Some(Confirm {
        prompt: format!("Archive all {} in {}?", ids.len(), group.label()),
        action: Pending::Archive { ids, label },
    });
    Vec::new()
}

/// One request for the whole batch, plus the undo the toast advertises.
fn archive(app: &mut App, ids: Vec<String>, label: &str) -> Vec<Effect> {
    app.row_actions.undo =
        Some(Undo { ids: ids.clone(), expires_ms: app.clock_ms.saturating_add(UNDO_MS) });
    app.row_actions.leave_select();
    app.toast(Level::Info, format!("archived {label} — u undo"));
    vec![Effect::ArchiveSessions { ids, archived: true }]
}

fn undo(app: &mut App) -> Vec<Effect> {
    let Some(undo) = app.row_actions.undo.take() else { return Vec::new() };
    if undo.expires_ms <= app.clock_ms {
        return Vec::new();
    }
    app.toast(Level::Info, format!("restored {} sessions", undo.ids.len()));
    vec![Effect::ArchiveSessions { ids: undo.ids, archived: false }]
}

/// Drops an expired undo offer and any selected id the list no longer holds.
pub fn prune(app: &mut App) {
    if app.row_actions.undo.as_ref().is_some_and(|u| u.expires_ms <= app.clock_ms) {
        app.row_actions.undo = None;
    }
    if app.row_actions.selected.is_empty() {
        return;
    }
    let live: HashSet<&str> = app.sessions.iter().map(|s| s.id.as_str()).collect();
    app.row_actions.selected.retain(|id| live.contains(id.as_str()));
}

/// `V`: every visible row between the anchor and the cursor. A folded group
/// contributes nothing, because the rows it hides are not on screen.
fn range_to_anchor(app: &mut App) {
    let Some(cursor) = app.selected_session_id() else { return };
    let ids = visible_ids(app);
    let anchor = app.row_actions.anchor.clone().unwrap_or_else(|| cursor.clone());
    let Some(from) = ids.iter().position(|id| *id == anchor) else { return };
    let Some(to) = ids.iter().position(|id| *id == cursor) else { return };
    let (lo, hi) = if from <= to { (from, to) } else { (to, from) };
    app.row_actions.mode = true;
    app.row_actions.selected.extend(ids[lo..=hi].iter().cloned());
}

fn visible_ids(app: &App) -> Vec<String> {
    super::session_list::sessions_of(&app.list_rows()).iter().map(|s| s.id.clone()).collect()
}

/// The selection in the order the list shows it, so a batch request and the
/// undo that follows it agree on order.
fn selected_in_view_order(app: &App) -> Vec<String> {
    visible_ids(app).into_iter().filter(|id| app.row_actions.is_selected(id)).collect()
}

/// The group whose rows the cursor is inside.
fn section_of_selection(app: &App) -> Option<Group> {
    let selected = app.selected_session_id()?;
    let rows = app.list_rows();
    let mut current = None;
    for row in &rows {
        match row {
            Row::Header { group, .. } => current = Some(*group),
            Row::Session { session, .. } if session.id == selected => return current,
            _ => {}
        }
    }
    None
}

/// Every top-level session of a group, folded or not: `A` archives the section
/// the header counts, not just the rows that happen to be on screen.
fn section_ids(app: &App, group: Group) -> Vec<String> {
    app.sessions
        .iter()
        .filter(|s| super::session_list::group_of(s) == group)
        .map(|s| s.id.clone())
        .collect()
}

/// A row's name if it has one, else its project, else its id.
fn label_of(app: &App, session_id: &str) -> String {
    let Some(s) = app.sessions.iter().find(|s| s.id == session_id) else {
        return session_id.to_owned();
    };
    if let Some(name) = s.name.as_deref().filter(|n| !n.is_empty()) {
        return format!("\"{name}\"");
    }
    s.metadata
        .get("project_name")
        .and_then(serde_json::Value::as_str)
        .filter(|p| !p.is_empty())
        .map_or_else(|| session_id.to_owned(), |p| format!("\"{p}\""))
}

fn type_into(buffer: &mut String, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => buffer.push(c),
        KeyCode::Backspace => {
            buffer.pop();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{Pending, RowAction, UNDO_MS, prune};
    use crate::app::action::Effect;
    use crate::app::state::App;
    use crate::app::{Action, reduce};
    use crate::testsupport::{dispatched_session, pinned_session, session, subagent};

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
            session("s-c", "gamma", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn act(app: &mut App, action: RowAction) -> Vec<Effect> {
        reduce(app, Action::RowAction(action))
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn selected(app: &App) -> Vec<String> {
        let mut ids: Vec<String> = app.row_actions.selected.iter().cloned().collect();
        ids.sort();
        ids
    }

    #[test]
    fn p_pins_the_row_and_unpins_a_pinned_one() {
        let mut app = app();
        match act(&mut app, RowAction::TogglePin).as_slice() {
            [Effect::PinSessions { ids, pinned: true }] => assert_eq!(ids, &["s-a".to_owned()]),
            _ => panic!("expected a pin"),
        }
        assert!(app.toasts.latest().expect("a toast").text.contains("pinned"));

        app.sessions[0] = pinned_session("s-a", "alpha");
        match act(&mut app, RowAction::TogglePin).as_slice() {
            [Effect::PinSessions { pinned: false, .. }] => {}
            _ => panic!("expected an unpin"),
        }
    }

    #[test]
    fn renaming_edits_a_buffer_and_sends_the_trimmed_name() {
        let mut app = app();
        act(&mut app, RowAction::RenameStart);
        for c in ['f', 'i', 'x', 'q'] {
            act(&mut app, RowAction::RenameKey(key(c)));
        }
        act(&mut app, RowAction::RenameKey(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(app.row_actions.strip().as_deref(), Some(" rename: fix_"));

        match act(&mut app, RowAction::RenameCommit).as_slice() {
            [Effect::RenameSession { session_id, name }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(name, "fix");
            }
            _ => panic!("expected a rename"),
        }
        assert!(app.row_actions.rename.is_none());
    }

    #[test]
    fn renaming_starts_from_the_current_name_and_refuses_an_empty_one() {
        let mut app = app();
        app.sessions[0].name = Some("old".to_owned());
        act(&mut app, RowAction::RenameStart);
        assert_eq!(app.row_actions.strip().as_deref(), Some(" rename: old_"));

        for _ in 0..3 {
            act(
                &mut app,
                RowAction::RenameKey(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
            );
        }
        assert!(act(&mut app, RowAction::RenameCommit).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("must not be empty"));
    }

    #[test]
    fn escape_closes_the_rename_without_sending_it() {
        let mut app = app();
        act(&mut app, RowAction::RenameStart);
        act(&mut app, RowAction::RenameKey(key('z')));
        assert!(act(&mut app, RowAction::RenameCancel).is_empty());
        assert!(app.row_actions.rename.is_none());
    }

    #[test]
    fn x_archives_one_row_at_once_and_offers_undo() {
        let mut app = app();
        app.clock_ms = 1_000;
        match act(&mut app, RowAction::ArchiveOrUnarchive).as_slice() {
            [Effect::ArchiveSessions { ids, archived: true }] => {
                assert_eq!(ids, &["s-a".to_owned()]);
            }
            _ => panic!("expected an archive"),
        }
        let toast = app.toasts.latest().expect("a toast");
        assert_eq!(toast.text, "archived \"alpha\" — u undo");
        assert_eq!(app.row_actions.undo.as_ref().expect("an undo").expires_ms, 1_000 + UNDO_MS);

        match act(&mut app, RowAction::Undo).as_slice() {
            [Effect::ArchiveSessions { ids, archived: false }] => {
                assert_eq!(ids, &["s-a".to_owned()]);
            }
            _ => panic!("expected an unarchive"),
        }
        assert!(app.row_actions.undo.is_none(), "undo is offered once");
    }

    #[test]
    fn an_undo_offer_expires_with_its_toast() {
        let mut app = app();
        act(&mut app, RowAction::ArchiveOrUnarchive);
        app.clock_ms += UNDO_MS;
        assert!(act(&mut app, RowAction::Undo).is_empty(), "the offer is gone");

        act(&mut app, RowAction::ArchiveOrUnarchive);
        app.clock_ms += UNDO_MS;
        prune(&mut app);
        assert!(app.row_actions.undo.is_none());
    }

    #[test]
    fn x_on_an_archived_row_unarchives_it() {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "archived", "done")];
        app.update_aggregates();
        match act(&mut app, RowAction::ArchiveOrUnarchive).as_slice() {
            [Effect::ArchiveSessions { archived: false, ids }] => {
                assert_eq!(ids, &["s-a".to_owned()]);
            }
            _ => panic!("expected an unarchive"),
        }
        assert!(app.row_actions.undo.is_none(), "an unarchive needs no undo");
    }

    #[test]
    fn kill_asks_first_and_only_then_sends() {
        let mut app = app();
        assert!(act(&mut app, RowAction::KillStart).is_empty());
        assert_eq!(app.row_actions.strip().as_deref(), Some(" Kill \"alpha\"? y/N"));

        assert!(act(&mut app, RowAction::ConfirmNo).is_empty());
        assert!(app.row_actions.confirm.is_none());

        act(&mut app, RowAction::KillStart);
        match act(&mut app, RowAction::ConfirmYes).as_slice() {
            [Effect::KillSession { session_id }] => assert_eq!(session_id, "s-a"),
            _ => panic!("expected a kill"),
        }
    }

    #[test]
    fn space_enters_select_mode_and_toggles_the_row_under_the_cursor() {
        let mut app = app();
        act(&mut app, RowAction::ToggleSelect);
        assert!(app.row_actions.mode);
        assert_eq!(selected(&app), ["s-a"]);
        assert!(app.row_actions.strip().expect("a strip").contains("SELECT (1)"));

        act(&mut app, RowAction::ToggleSelect);
        assert!(selected(&app).is_empty());
        assert!(app.row_actions.mode, "the strip stays up with nothing selected");

        act(&mut app, RowAction::ClearSelection);
        assert!(!app.row_actions.mode);
        assert!(app.row_actions.strip().is_none());
    }

    #[test]
    fn a_range_covers_the_rows_between_the_anchor_and_the_cursor() {
        let mut app = app();
        act(&mut app, RowAction::ToggleSelect);
        reduce(&mut app, Action::SelectLast);
        act(&mut app, RowAction::RangeToAnchor);
        assert_eq!(selected(&app), ["s-a", "s-b", "s-c"]);
    }

    #[test]
    fn a_range_runs_backwards_too() {
        let mut app = app();
        reduce(&mut app, Action::SelectLast);
        act(&mut app, RowAction::ToggleSelect);
        reduce(&mut app, Action::SelectFirst);
        act(&mut app, RowAction::RangeToAnchor);
        assert_eq!(selected(&app), ["s-a", "s-b", "s-c"]);
    }

    /// Mirrors the webui `rangeIds` test: the range walks what is on screen.
    #[test]
    fn a_range_over_a_folded_group_takes_only_the_visible_rows() {
        let mut app = App::new();
        app.sessions = vec![session("s-p", "parent", "active", "working")];
        for i in 0..12 {
            app.sessions.push(subagent(&format!("s-c{i:02}"), "s-p", "lane"));
        }
        app.sessions.push(session("s-z", "last", "active", "done"));
        app.update_aggregates();

        act(&mut app, RowAction::ToggleSelect);
        reduce(&mut app, Action::SelectLast);
        act(&mut app, RowAction::RangeToAnchor);
        assert_eq!(
            selected(&app),
            ["s-p", "s-z"],
            "the twelve folded children are not on screen, so they are not in the range"
        );
    }

    #[test]
    fn star_selects_every_visible_row() {
        let mut app = app();
        act(&mut app, RowAction::SelectAllVisible);
        assert_eq!(selected(&app), ["s-a", "s-b", "s-c"]);
    }

    #[test]
    fn a_batch_archive_confirms_once_and_sends_one_request() {
        let mut app = app();
        act(&mut app, RowAction::SelectAllVisible);
        assert!(act(&mut app, RowAction::ArchiveOrUnarchive).is_empty(), "it asks first");
        assert_eq!(app.row_actions.strip().as_deref(), Some(" Archive 3 sessions? y/N"));

        match act(&mut app, RowAction::ConfirmYes).as_slice() {
            [Effect::ArchiveSessions { ids, archived: true }] => {
                assert_eq!(ids, &["s-a".to_owned(), "s-b".to_owned(), "s-c".to_owned()]);
            }
            _ => panic!("expected one batch archive"),
        }
        assert!(!app.row_actions.mode, "the batch leaves select mode");
        assert!(app.toasts.latest().expect("a toast").text.contains("3 sessions — u undo"));
    }

    #[test]
    fn declining_the_batch_keeps_the_selection() {
        let mut app = app();
        act(&mut app, RowAction::SelectAllVisible);
        act(&mut app, RowAction::ArchiveOrUnarchive);
        act(&mut app, RowAction::ConfirmNo);
        assert_eq!(selected(&app), ["s-a", "s-b", "s-c"]);
    }

    #[test]
    fn escape_answers_the_confirm_before_it_leaves_select_mode() {
        let mut app = app();
        act(&mut app, RowAction::SelectAllVisible);
        act(&mut app, RowAction::ArchiveOrUnarchive);
        act(&mut app, RowAction::ClearSelection);
        assert!(app.row_actions.confirm.is_none());
        assert_eq!(selected(&app), ["s-a", "s-b", "s-c"], "one key, one job");
    }

    #[test]
    fn capital_a_archives_the_whole_section_the_cursor_is_in() {
        let mut app = App::new();
        app.sessions = vec![
            session("s-w", "work", "active", "working"),
            dispatched_session("s-d1", "worker-one", "working"),
            dispatched_session("s-d2", "worker-two", "working"),
        ];
        app.update_aggregates();
        reduce(&mut app, Action::SelectLast);

        assert!(act(&mut app, RowAction::ArchiveSection).is_empty());
        assert_eq!(app.row_actions.strip().as_deref(), Some(" Archive all 2 in Dispatched? y/N"));
        match act(&mut app, RowAction::ConfirmYes).as_slice() {
            [Effect::ArchiveSessions { ids, archived: true }] => {
                assert_eq!(ids.len(), 2);
                assert!(ids.contains(&"s-d1".to_owned()) && ids.contains(&"s-d2".to_owned()));
            }
            _ => panic!("expected the section archive"),
        }
    }

    #[test]
    fn a_folded_section_still_archives_every_row_it_counts() {
        let mut app = App::new();
        app.sessions = vec![
            dispatched_session("s-d1", "worker-one", "working"),
            dispatched_session("s-d2", "worker-two", "working"),
        ];
        app.update_aggregates();
        act(&mut app, RowAction::ArchiveSection);
        let Some(confirm) = app.row_actions.confirm.clone() else { panic!("expected a confirm") };
        let Pending::Archive { ids, .. } = confirm.action else { panic!("expected an archive") };
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn the_selection_survives_the_rows_reordering_under_it() {
        let mut app = app();
        act(&mut app, RowAction::ToggleSelect);
        reduce(&mut app, Action::SelectNext);
        act(&mut app, RowAction::ToggleSelect);
        assert_eq!(selected(&app), ["s-a", "s-b"]);

        // A live update reorders the list and drops one of the selected rows.
        app.sessions = vec![
            session("s-c", "gamma", "active", "blocked"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        prune(&mut app);
        assert_eq!(selected(&app), ["s-b"], "ids follow the row, not the slot");

        act(&mut app, RowAction::ArchiveOrUnarchive);
        assert_eq!(app.row_actions.strip().as_deref(), Some(" Archive 1 sessions? y/N"));
    }

    #[test]
    fn nothing_acts_on_an_empty_list() {
        let mut app = App::new();
        for action in [
            RowAction::TogglePin,
            RowAction::RenameStart,
            RowAction::ArchiveOrUnarchive,
            RowAction::KillStart,
            RowAction::ArchiveSection,
            RowAction::ToggleSelect,
            RowAction::RangeToAnchor,
            RowAction::SelectAllVisible,
            RowAction::Undo,
            RowAction::ConfirmYes,
        ] {
            assert!(act(&mut app, action).is_empty());
        }
        assert!(app.row_actions.strip().is_none());
    }
}
