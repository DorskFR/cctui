//! Row actions on the selected session list row: pin, rename, archive, kill.
//!
//! Everything here is keyed by session id, never by row index: a websocket
//! event reorders the list under the cursor, and an index would then act on
//! whatever slid into that slot.

use crossterm::event::{KeyCode, KeyEvent};

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// How long the undo offer stands, matching the toast it rides on.
pub const UNDO_MS: i64 = super::toast::Toasts::TTL_MS;

/// What a `y` answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pending {
    Kill(String),
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
    pub confirm: Option<Confirm>,
    pub rename: Option<Rename>,
    pub undo: Option<Undo>,
}

impl RowActionState {
    /// The strip's own text, or `None` when the list is in its normal state.
    #[must_use]
    pub fn strip(&self) -> Option<String> {
        if let Some(confirm) = &self.confirm {
            return Some(format!(" {} y/N", confirm.prompt));
        }
        let rename = self.rename.as_ref()?;
        Some(format!(" rename: {}_", rename.input))
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RowAction {
    TogglePin,
    RenameStart,
    RenameKey(KeyEvent),
    RenameCommit,
    RenameCancel,
    ArchiveOrUnarchive,
    KillStart,
    ConfirmYes,
    ConfirmNo,
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
        RowAction::ConfirmYes => {
            let Some(confirm) = app.row_actions.confirm.take() else { return Vec::new() };
            let Pending::Kill(session_id) = confirm.action;
            vec![Effect::KillSession { session_id }]
        }
        RowAction::ConfirmNo => {
            app.row_actions.confirm = None;
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

/// One row acts at once: the undo toast is the confirmation.
fn archive_or_unarchive(app: &mut App) -> Vec<Effect> {
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

/// One request for the whole batch, plus the undo the toast advertises.
fn archive(app: &mut App, ids: Vec<String>, label: &str) -> Vec<Effect> {
    app.row_actions.undo =
        Some(Undo { ids: ids.clone(), expires_ms: app.clock_ms.saturating_add(UNDO_MS) });
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

/// Drops an undo offer the clock has run out on.
pub fn prune(app: &mut App) {
    if app.row_actions.undo.as_ref().is_some_and(|u| u.expires_ms <= app.clock_ms) {
        app.row_actions.undo = None;
    }
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

    use super::{RowAction, UNDO_MS, prune};
    use crate::app::action::Effect;
    use crate::app::state::App;
    use crate::app::{Action, reduce};
    use crate::testsupport::{pinned_session, session};

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
    fn nothing_acts_on_an_empty_list() {
        let mut app = App::new();
        for action in [
            RowAction::TogglePin,
            RowAction::RenameStart,
            RowAction::ArchiveOrUnarchive,
            RowAction::KillStart,
            RowAction::Undo,
            RowAction::ConfirmYes,
        ] {
            assert!(act(&mut app, action).is_empty());
        }
        assert!(app.row_actions.strip().is_none());
    }
}
