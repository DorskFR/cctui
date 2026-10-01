//! Labels: the catalogue, the `l` picker, the `L` filter.
//!
//! Hues come from `cctui_clientcore::labels`, so a label reads as the same
//! colour here and in the webui; nothing in this module invents one.

use std::collections::BTreeSet;

use cctui_proto::api::Label;

use super::action::Effect;
use super::state::App;
use super::toast::Level;

/// What the `l` overlay is doing. Creating and renaming both type a name, and
/// deleting waits for a yes, so the overlay is a small state machine rather than
/// three separate views.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerMode {
    /// Browsing, with `filter` narrowing the list.
    Browse,
    /// Typing a name for a new label.
    Create { name: String },
    /// Renaming the selected label; the hue is picked separately.
    Rename { id: String, name: String },
    /// Choosing one of the twelve preset hues, or Auto.
    Hue { id: String, index: usize },
    /// `d` asked, waiting for `y`.
    ConfirmDelete { id: String, name: String },
}

/// The `l` overlay's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LabelPicker {
    /// The session whose labels are being edited.
    pub session_id: String,
    pub filter: String,
    pub selected: usize,
    pub mode: PickerMode,
}

/// The catalogue, the open overlay and the active filter.
#[derive(Debug, Default)]
pub struct Labels {
    /// Every label the caller owns, by name.
    pub all: Vec<Label>,
    pub picker: Option<LabelPicker>,
    /// `L`'s any-of filter, by label id.
    pub filter: BTreeSet<String>,
    /// Whether the `L` filter overlay is open.
    pub filter_open: bool,
    pub filter_selected: usize,
}

impl Labels {
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Label> {
        self.all.iter().find(|l| l.id == id)
    }

    /// The catalogue narrowed by a fuzzy query, in catalogue order.
    #[must_use]
    pub fn matching(&self, query: &str) -> Vec<&Label> {
        self.all.iter().filter(|l| fuzzy_matches(&l.name, query)).collect()
    }

    /// Whether `session` passes the active filter: no filter passes everything,
    /// otherwise any one of its labels is enough.
    #[must_use]
    pub fn passes(&self, session: &cctui_proto::api::SessionListItem) -> bool {
        self.filter.is_empty() || session.labels.iter().any(|l| self.filter.contains(&l.id))
    }

    /// The filter as the status line words it, or `None` when nothing is filtered.
    #[must_use]
    pub fn filter_text(&self) -> Option<String> {
        if self.filter.is_empty() {
            return None;
        }
        let mut names: Vec<&str> =
            self.filter.iter().map(|id| self.get(id).map_or("?", |l| l.name.as_str())).collect();
        names.sort_unstable();
        Some(format!("labels: {}", names.join(", ")))
    }

    /// Drop filter entries whose label is gone, so a deleted label does not keep
    /// hiding rows forever.
    fn prune_filter(&mut self) {
        let known: BTreeSet<String> = self.all.iter().map(|l| l.id.clone()).collect();
        self.filter.retain(|id| known.contains(id));
    }
}

/// Subsequence match, case-insensitive: `wa5` finds `wave-5`.
#[must_use]
pub fn fuzzy_matches(name: &str, query: &str) -> bool {
    if query.is_empty() {
        return true;
    }
    let mut haystack = name.chars().flat_map(char::to_lowercase);
    query.chars().flat_map(char::to_lowercase).all(|needle| haystack.any(|c| c == needle))
}

pub enum LabelAction {
    /// A key the overlay has to read in the light of its mode: the same letter
    /// narrows the list while browsing and types a name while creating.
    Key(crossterm::event::KeyEvent),
    /// `esc`: out of a sub-mode, or out of the overlay when already browsing.
    CloseOrCancel,
    /// `space`: toggle while browsing, a space in a name otherwise.
    ToggleOrSpace,
    /// The catalogue arrived from the server.
    Loaded(Vec<Label>),
    /// `l` on a row.
    OpenPicker,
    ClosePicker,
    FilterKey(char),
    FilterBackspace,
    SelectNext,
    SelectPrev,
    /// `space`: attach the selected label, or detach it when already attached.
    Toggle,
    /// The server confirmed an attach or detach.
    Attached {
        session_id: String,
        label_id: String,
    },
    Detached {
        session_id: String,
        label_id: String,
    },
    /// `c`, `e`, `d`.
    StartCreate,
    StartRename,
    StartDelete,
    /// Typing a name in create or rename mode.
    NameKey(char),
    NameBackspace,
    /// `enter` in create/rename, or `y` in confirm-delete.
    Commit,
    /// `esc` out of a sub-mode, back to browsing.
    Cancel,
    /// Hue picker movement and choice.
    HueNext,
    HuePrev,
    /// `L`.
    OpenFilter,
    CloseFilter,
    FilterSelectNext,
    FilterSelectPrev,
    FilterToggle,
    FilterClear,
}

#[allow(clippy::too_many_lines)]
pub fn reduce_labels(app: &mut App, action: LabelAction) -> Vec<Effect> {
    match action {
        LabelAction::Key(key) => {
            key_in_mode(app, key);
            Vec::new()
        }
        LabelAction::CloseOrCancel => {
            if app.labels.filter_open {
                return reduce_labels(app, LabelAction::CloseFilter);
            }
            let browsing = app.labels.picker.as_ref().is_some_and(|p| p.mode == PickerMode::Browse);
            if browsing {
                reduce_labels(app, LabelAction::ClosePicker)
            } else {
                reduce_labels(app, LabelAction::Cancel)
            }
        }
        LabelAction::ToggleOrSpace => {
            if app.labels.filter_open {
                return reduce_labels(app, LabelAction::FilterToggle);
            }
            let browsing = app.labels.picker.as_ref().is_some_and(|p| p.mode == PickerMode::Browse);
            if browsing {
                reduce_labels(app, LabelAction::Toggle)
            } else {
                reduce_labels(
                    app,
                    LabelAction::Key(crossterm::event::KeyEvent::new(
                        crossterm::event::KeyCode::Char(' '),
                        crossterm::event::KeyModifiers::NONE,
                    )),
                )
            }
        }
        LabelAction::Loaded(labels) => {
            app.labels.all = labels;
            app.labels.all.sort_by(|a, b| a.name.cmp(&b.name));
            app.labels.prune_filter();
            clamp_selection(app);
            vec![Effect::SaveUiState(app.ui.clone())]
        }
        LabelAction::OpenPicker => open_picker(app),
        LabelAction::ClosePicker => {
            if app.labels.picker.take().is_some() {
                app.router.pop();
            }
            Vec::new()
        }
        LabelAction::FilterKey(c) => {
            if let Some(picker) = browsing_mut(app) {
                picker.filter.push(c);
                picker.selected = 0;
            }
            Vec::new()
        }
        LabelAction::FilterBackspace => {
            if let Some(picker) = browsing_mut(app) {
                picker.filter.pop();
                picker.selected = 0;
            }
            Vec::new()
        }
        // The two overlays share the movement keys; which list moves is whichever
        // is open.
        LabelAction::SelectNext => {
            if app.labels.filter_open {
                return reduce_labels(app, LabelAction::FilterSelectNext);
            }
            move_selection(app, 1);
            Vec::new()
        }
        LabelAction::SelectPrev => {
            if app.labels.filter_open {
                return reduce_labels(app, LabelAction::FilterSelectPrev);
            }
            move_selection(app, -1);
            Vec::new()
        }
        LabelAction::Toggle => toggle_selected(app),
        LabelAction::Attached { session_id, label_id } => {
            let Some(label) = app.labels.get(&label_id).cloned() else { return Vec::new() };
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == session_id)
                && !s.labels.iter().any(|l| l.id == label_id)
            {
                s.labels.push(label);
            }
            Vec::new()
        }
        LabelAction::Detached { session_id, label_id } => {
            if let Some(s) = app.sessions.iter_mut().find(|s| s.id == session_id) {
                s.labels.retain(|l| l.id != label_id);
            }
            Vec::new()
        }
        LabelAction::StartCreate => {
            if let Some(picker) = app.labels.picker.as_mut() {
                // The query is usually the name you were about to create.
                let name = picker.filter.clone();
                picker.mode = PickerMode::Create { name };
            }
            Vec::new()
        }
        LabelAction::StartRename => {
            let Some(label) = selected_label(app).cloned() else { return Vec::new() };
            if let Some(picker) = app.labels.picker.as_mut() {
                picker.mode = PickerMode::Rename { id: label.id, name: label.name };
            }
            Vec::new()
        }
        LabelAction::StartDelete => {
            let Some(label) = selected_label(app).cloned() else { return Vec::new() };
            if let Some(picker) = app.labels.picker.as_mut() {
                picker.mode = PickerMode::ConfirmDelete { id: label.id, name: label.name };
            }
            Vec::new()
        }
        LabelAction::NameKey(c) => {
            if let Some(picker) = app.labels.picker.as_mut() {
                match &mut picker.mode {
                    PickerMode::Create { name } | PickerMode::Rename { name, .. } => name.push(c),
                    _ => {}
                }
            }
            Vec::new()
        }
        LabelAction::NameBackspace => {
            if let Some(picker) = app.labels.picker.as_mut() {
                match &mut picker.mode {
                    PickerMode::Create { name } | PickerMode::Rename { name, .. } => {
                        name.pop();
                    }
                    _ => {}
                }
            }
            Vec::new()
        }
        LabelAction::Commit => commit(app),
        LabelAction::Cancel => {
            if let Some(picker) = app.labels.picker.as_mut() {
                picker.mode = PickerMode::Browse;
                return Vec::new();
            }
            Vec::new()
        }
        LabelAction::HueNext => {
            move_hue(app, 1);
            Vec::new()
        }
        LabelAction::HuePrev => {
            move_hue(app, -1);
            Vec::new()
        }
        LabelAction::OpenFilter => {
            if app.labels.all.is_empty() {
                app.toast(Level::Info, "no labels yet — press l on a row to make one");
                return Vec::new();
            }
            app.labels.filter_open = true;
            app.labels.filter_selected = 0;
            app.router.push(super::state::View::LabelFilter);
            Vec::new()
        }
        LabelAction::CloseFilter => {
            if std::mem::take(&mut app.labels.filter_open) {
                app.router.pop();
            }
            Vec::new()
        }
        LabelAction::FilterSelectNext => {
            let len = app.labels.all.len();
            if len > 0 {
                app.labels.filter_selected = (app.labels.filter_selected + 1).min(len - 1);
            }
            Vec::new()
        }
        LabelAction::FilterSelectPrev => {
            app.labels.filter_selected = app.labels.filter_selected.saturating_sub(1);
            Vec::new()
        }
        LabelAction::FilterToggle => {
            let Some(label) = app.labels.all.get(app.labels.filter_selected).cloned() else {
                return Vec::new();
            };
            if !app.labels.filter.remove(&label.id) {
                app.labels.filter.insert(label.id);
            }
            app.ui.label_filter = app.labels.filter.clone();
            clamp_list_selection(app);
            vec![Effect::SaveUiState(app.ui.clone())]
        }
        LabelAction::FilterClear => {
            app.labels.filter.clear();
            app.ui.label_filter.clear();
            clamp_list_selection(app);
            vec![Effect::SaveUiState(app.ui.clone())]
        }
    }
}

/// What a bare key means depends on the mode: the overlay is modal, so it reads
/// every key rather than leaving most of them to a binding table.
fn key_in_mode(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::KeyCode;
    let Some(mode) = app.labels.picker.as_ref().map(|p| p.mode.clone()) else { return };
    match mode {
        PickerMode::Browse => match key.code {
            KeyCode::Char(c)
                if !key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                let _ = reduce_labels(app, LabelAction::FilterKey(c));
            }
            KeyCode::Backspace => {
                let _ = reduce_labels(app, LabelAction::FilterBackspace);
            }
            _ => {}
        },
        PickerMode::Create { .. } | PickerMode::Rename { .. } => match key.code {
            KeyCode::Char(c)
                if !key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                let _ = reduce_labels(app, LabelAction::NameKey(c));
            }
            KeyCode::Backspace => {
                let _ = reduce_labels(app, LabelAction::NameBackspace);
            }
            _ => {}
        },
        PickerMode::Hue { .. } => match key.code {
            KeyCode::Right | KeyCode::Char('l') => {
                let _ = reduce_labels(app, LabelAction::HueNext);
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let _ = reduce_labels(app, LabelAction::HuePrev);
            }
            _ => {}
        },
        PickerMode::ConfirmDelete { .. } => match key.code {
            KeyCode::Char('y') => {
                let _ = reduce_labels(app, LabelAction::Commit);
            }
            KeyCode::Char('n') => {
                let _ = reduce_labels(app, LabelAction::Cancel);
            }
            _ => {}
        },
    }
}

fn open_picker(app: &mut App) -> Vec<Effect> {
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    app.labels.picker = Some(LabelPicker {
        session_id,
        filter: String::new(),
        selected: 0,
        mode: PickerMode::Browse,
    });
    app.router.push(super::state::View::LabelPicker);
    // The catalogue may be stale: another client can have added one.
    vec![Effect::FetchLabels]
}

/// The picker, only while it is browsing — the sub-modes own the keyboard.
fn browsing_mut(app: &mut App) -> Option<&mut LabelPicker> {
    app.labels.picker.as_mut().filter(|p| p.mode == PickerMode::Browse)
}

fn visible_len(app: &App) -> usize {
    let Some(picker) = app.labels.picker.as_ref() else { return 0 };
    app.labels.matching(&picker.filter).len()
}

fn move_selection(app: &mut App, delta: i32) {
    let len = visible_len(app);
    let Some(picker) = browsing_mut(app) else { return };
    if len == 0 {
        picker.selected = 0;
        return;
    }
    let next = if delta < 0 {
        picker.selected.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        picker.selected.saturating_add(delta.unsigned_abs() as usize)
    };
    picker.selected = next.min(len - 1);
}

fn clamp_selection(app: &mut App) {
    let len = visible_len(app);
    if let Some(picker) = app.labels.picker.as_mut() {
        picker.selected = if len == 0 { 0 } else { picker.selected.min(len - 1) };
    }
    let catalogue = app.labels.all.len();
    app.labels.filter_selected =
        if catalogue == 0 { 0 } else { app.labels.filter_selected.min(catalogue - 1) };
}

/// A narrowed list can be shorter than the cursor; bring the row cursor back in.
fn clamp_list_selection(app: &mut App) {
    let len = app.flattened_sessions().len();
    app.selected_index = if len == 0 { 0 } else { app.selected_index.min(len - 1) };
}

fn selected_label(app: &App) -> Option<&Label> {
    let picker = app.labels.picker.as_ref()?;
    app.labels.matching(&picker.filter).get(picker.selected).copied()
}

fn toggle_selected(app: &App) -> Vec<Effect> {
    let Some(picker) = app.labels.picker.as_ref() else { return Vec::new() };
    let session_id = picker.session_id.clone();
    let Some(label) = selected_label(app).cloned() else { return Vec::new() };
    let attached = app
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .is_some_and(|s| s.labels.iter().any(|l| l.id == label.id));
    if attached {
        vec![Effect::DetachLabel { session_id, label_id: label.id }]
    } else {
        vec![Effect::AttachLabel { session_id, label_id: label.id }]
    }
}

const fn move_hue(app: &mut App, delta: i32) {
    let Some(picker) = app.labels.picker.as_mut() else { return };
    let PickerMode::Hue { index, .. } = &mut picker.mode else { return };
    // One past the presets is Auto, so the ring is 13 long and wraps both ways.
    let len = cctui_clientcore::labels::LABEL_HUES.len() + 1;
    *index = if delta < 0 {
        let back = delta.unsigned_abs() as usize % len;
        (*index + len - back) % len
    } else {
        (*index + delta.unsigned_abs() as usize) % len
    };
}

/// `enter` in a sub-mode. Creating and renaming hand off to the hue picker, so a
/// new label gets a colour in the same gesture.
fn commit(app: &mut App) -> Vec<Effect> {
    let Some((mode, session_id)) =
        app.labels.picker.as_ref().map(|p| (p.mode.clone(), p.session_id.clone()))
    else {
        return Vec::new();
    };
    match mode {
        PickerMode::Browse => Vec::new(),
        PickerMode::Create { name } => {
            let name = name.trim().to_owned();
            if name.is_empty() {
                app.toast(Level::Warn, "a label needs a name");
                return Vec::new();
            }
            if let Some(picker) = app.labels.picker.as_mut() {
                picker.mode = PickerMode::Hue { id: String::new(), index: auto_index() };
                picker.filter = name;
            }
            Vec::new()
        }
        PickerMode::Rename { id, name } => {
            let name = name.trim().to_owned();
            if name.is_empty() {
                app.toast(Level::Warn, "a label needs a name");
                return Vec::new();
            }
            if let Some(picker) = app.labels.picker.as_mut() {
                picker.mode = PickerMode::Hue { id: id.clone(), index: auto_index() };
            }
            vec![Effect::UpdateLabel { id, name: Some(name), color: None }]
        }
        PickerMode::Hue { id, index } => {
            let hue = cctui_clientcore::labels::LABEL_HUES.get(index).copied();
            let color = cctui_clientcore::labels::hue_to_color(hue);
            let name = app.labels.picker.as_ref().map(|p| p.filter.clone()).unwrap_or_default();
            if let Some(picker) = app.labels.picker.as_mut() {
                picker.mode = PickerMode::Browse;
                picker.filter = String::new();
            }
            if id.is_empty() {
                vec![Effect::CreateLabel { name, color, session_id }]
            } else {
                vec![Effect::UpdateLabel { id, name: None, color: Some(color) }]
            }
        }
        PickerMode::ConfirmDelete { id, name } => {
            if let Some(picker) = app.labels.picker.as_mut() {
                picker.mode = PickerMode::Browse;
            }
            app.toast(Level::Info, format!("deleted {name}"));
            vec![Effect::DeleteLabel { id }]
        }
    }
}

/// The index that means Auto: one past the last preset.
#[must_use]
pub const fn auto_index() -> usize {
    cctui_clientcore::labels::LABEL_HUES.len()
}

#[cfg(test)]
mod tests {
    use super::{LabelAction, PickerMode, fuzzy_matches};
    use crate::app::action::Effect;
    use crate::app::{Action, App, View, reduce};
    use crate::testsupport::session;

    fn label(id: &str, name: &str, color: &str) -> cctui_proto::api::Label {
        cctui_proto::api::Label {
            id: id.to_owned(),
            name: name.to_owned(),
            color: color.to_owned(),
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn with_labels() -> App {
        let mut app = app();
        let _ = reduce(
            &mut app,
            Action::Labels(LabelAction::Loaded(vec![
                label("l-5", "wave-5", ""),
                label("l-6", "wave-6", "210"),
                label("l-i", "infra", ""),
            ])),
        );
        app
    }

    fn act(app: &mut App, action: LabelAction) -> Vec<Effect> {
        reduce(app, Action::Labels(action))
    }

    #[test]
    fn a_fuzzy_query_matches_a_subsequence_case_blind() {
        assert!(fuzzy_matches("wave-5", ""));
        assert!(fuzzy_matches("wave-5", "wa"));
        assert!(fuzzy_matches("wave-5", "w5"));
        assert!(fuzzy_matches("wave-5", "WAVE"));
        assert!(!fuzzy_matches("wave-5", "wx"));
        assert!(!fuzzy_matches("wave-5", "5w"), "order matters");
    }

    #[test]
    fn the_catalogue_is_sorted_and_narrowed_by_the_query() {
        let app = with_labels();
        let names: Vec<&str> = app.labels.all.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, ["infra", "wave-5", "wave-6"]);
        let hit: Vec<&str> = app.labels.matching("wa").iter().map(|l| l.name.as_str()).collect();
        assert_eq!(hit, ["wave-5", "wave-6"]);
    }

    #[test]
    fn opening_the_picker_refetches_the_catalogue() {
        let mut app = with_labels();
        let effects = act(&mut app, LabelAction::OpenPicker);
        assert!(effects.iter().any(|e| matches!(e, Effect::FetchLabels)));
        assert_eq!(app.view(), View::LabelPicker);
        assert_eq!(app.labels.picker.as_ref().expect("open").session_id, "s-a");

        act(&mut app, LabelAction::ClosePicker);
        assert!(app.labels.picker.is_none());
        assert_eq!(app.view(), View::SessionList);
    }

    #[test]
    fn space_attaches_then_detaches_the_selected_label() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        match act(&mut app, LabelAction::Toggle).as_slice() {
            [Effect::AttachLabel { session_id, label_id }] => {
                assert_eq!(session_id, "s-a");
                assert_eq!(label_id, "l-i", "the first label by name");
            }
            other => panic!("expected an attach, got {}", other.len()),
        }
        act(
            &mut app,
            LabelAction::Attached { session_id: "s-a".to_owned(), label_id: "l-i".to_owned() },
        );
        assert_eq!(app.sessions[0].labels.len(), 1);

        assert!(matches!(
            act(&mut app, LabelAction::Toggle).as_slice(),
            [Effect::DetachLabel { .. }]
        ));
        act(
            &mut app,
            LabelAction::Detached { session_id: "s-a".to_owned(), label_id: "l-i".to_owned() },
        );
        assert!(app.sessions[0].labels.is_empty());
    }

    #[test]
    fn attaching_the_same_label_twice_does_not_duplicate_the_chip() {
        let mut app = with_labels();
        for _ in 0..2 {
            act(
                &mut app,
                LabelAction::Attached { session_id: "s-a".to_owned(), label_id: "l-5".to_owned() },
            );
        }
        assert_eq!(app.sessions[0].labels.len(), 1);
    }

    #[test]
    fn typing_narrows_the_picker_and_resets_the_cursor() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        act(&mut app, LabelAction::SelectNext);
        act(&mut app, LabelAction::SelectNext);
        assert_eq!(app.labels.picker.as_ref().expect("open").selected, 2);
        act(&mut app, LabelAction::FilterKey('w'));
        let picker = app.labels.picker.as_ref().expect("open");
        assert_eq!(picker.filter, "w");
        assert_eq!(picker.selected, 0);
        act(&mut app, LabelAction::FilterBackspace);
        assert_eq!(app.labels.picker.as_ref().expect("open").filter, "");
    }

    #[test]
    fn the_cursor_never_leaves_the_narrowed_list() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        for _ in 0..9 {
            act(&mut app, LabelAction::SelectNext);
        }
        assert_eq!(app.labels.picker.as_ref().expect("open").selected, 2);
        act(&mut app, LabelAction::FilterKey('i'));
        assert_eq!(
            app.labels.picker.as_ref().expect("open").selected,
            0,
            "one match left, so the cursor is on it"
        );
    }

    #[test]
    fn creating_seeds_the_name_from_the_query_then_asks_for_a_hue() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        act(&mut app, LabelAction::FilterKey('w'));
        act(&mut app, LabelAction::FilterKey('x'));
        act(&mut app, LabelAction::StartCreate);
        assert_eq!(
            app.labels.picker.as_ref().expect("open").mode,
            PickerMode::Create { name: "wx".to_owned() },
            "the query you typed is the name you meant"
        );

        act(&mut app, LabelAction::NameKey('!'));
        act(&mut app, LabelAction::NameBackspace);
        assert!(act(&mut app, LabelAction::Commit).is_empty(), "the hue comes first");
        assert!(matches!(app.labels.picker.as_ref().expect("open").mode, PickerMode::Hue { .. }));

        match act(&mut app, LabelAction::Commit).as_slice() {
            [Effect::CreateLabel { name, color, session_id }] => {
                assert_eq!(name, "wx");
                assert_eq!(color, "", "Auto is the default hue");
                assert_eq!(session_id, "s-a");
            }
            other => panic!("expected a create, got {}", other.len()),
        }
        assert_eq!(app.labels.picker.as_ref().expect("open").mode, PickerMode::Browse);
    }

    #[test]
    fn a_nameless_label_is_refused_rather_than_created() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        act(&mut app, LabelAction::StartCreate);
        assert!(act(&mut app, LabelAction::Commit).is_empty());
        assert_eq!(
            app.labels.picker.as_ref().expect("open").mode,
            PickerMode::Create { name: String::new() },
            "still asking for a name"
        );
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn a_chosen_preset_hue_is_persisted_as_its_number() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        act(&mut app, LabelAction::StartCreate);
        act(&mut app, LabelAction::NameKey('q'));
        act(&mut app, LabelAction::Commit);
        // Auto is one past the presets, so one step forward lands on the first.
        act(&mut app, LabelAction::HueNext);
        match act(&mut app, LabelAction::Commit).as_slice() {
            [Effect::CreateLabel { color, .. }] => assert_eq!(color, "0"),
            other => panic!("expected a create, got {}", other.len()),
        }
    }

    #[test]
    fn the_hue_ring_wraps_both_ways() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        act(&mut app, LabelAction::StartCreate);
        act(&mut app, LabelAction::NameKey('q'));
        act(&mut app, LabelAction::Commit);
        act(&mut app, LabelAction::HuePrev);
        match act(&mut app, LabelAction::Commit).as_slice() {
            [Effect::CreateLabel { color, .. }] => assert_eq!(color, "330", "the last preset"),
            other => panic!("expected a create, got {}", other.len()),
        }
    }

    #[test]
    fn renaming_sends_the_name_and_then_offers_the_hue() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        act(&mut app, LabelAction::StartRename);
        act(&mut app, LabelAction::NameKey('2'));
        match act(&mut app, LabelAction::Commit).as_slice() {
            [Effect::UpdateLabel { id, name, color }] => {
                assert_eq!(id, "l-i");
                assert_eq!(name.as_deref(), Some("infra2"));
                assert_eq!(*color, None);
            }
            other => panic!("expected an update, got {}", other.len()),
        }
        match act(&mut app, LabelAction::Commit).as_slice() {
            [Effect::UpdateLabel { id, name, color }] => {
                assert_eq!(id, "l-i");
                assert_eq!(*name, None);
                assert_eq!(color.as_deref(), Some(""));
            }
            other => panic!("expected a recolor, got {}", other.len()),
        }
    }

    #[test]
    fn deleting_waits_for_a_confirmation() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenPicker);
        act(&mut app, LabelAction::StartDelete);
        assert_eq!(
            app.labels.picker.as_ref().expect("open").mode,
            PickerMode::ConfirmDelete { id: "l-i".to_owned(), name: "infra".to_owned() }
        );
        act(&mut app, LabelAction::Cancel);
        assert_eq!(app.labels.picker.as_ref().expect("open").mode, PickerMode::Browse);

        act(&mut app, LabelAction::StartDelete);
        match act(&mut app, LabelAction::Commit).as_slice() {
            [Effect::DeleteLabel { id }] => assert_eq!(id, "l-i"),
            other => panic!("expected a delete, got {}", other.len()),
        }
    }

    #[test]
    fn the_filter_is_any_of_and_shows_in_the_status_line() {
        let mut app = with_labels();
        assert_eq!(app.labels.filter_text(), None);
        act(&mut app, LabelAction::OpenFilter);
        assert_eq!(app.view(), View::LabelFilter);

        act(&mut app, LabelAction::FilterToggle);
        assert_eq!(app.labels.filter_text().as_deref(), Some("labels: infra"));
        act(&mut app, LabelAction::FilterSelectNext);
        act(&mut app, LabelAction::FilterToggle);
        assert_eq!(app.labels.filter_text().as_deref(), Some("labels: infra, wave-5"));

        act(&mut app, LabelAction::FilterToggle);
        assert_eq!(app.labels.filter_text().as_deref(), Some("labels: infra"));
        act(&mut app, LabelAction::FilterClear);
        assert_eq!(app.labels.filter_text(), None);

        act(&mut app, LabelAction::CloseFilter);
        assert_eq!(app.view(), View::SessionList);
    }

    #[test]
    fn a_filtered_list_only_keeps_sessions_carrying_one_of_the_labels() {
        let mut app = with_labels();
        act(
            &mut app,
            LabelAction::Attached { session_id: "s-a".to_owned(), label_id: "l-5".to_owned() },
        );
        act(&mut app, LabelAction::OpenFilter);
        // `wave-5` is second by name, after `infra`.
        act(&mut app, LabelAction::FilterSelectNext);
        act(&mut app, LabelAction::FilterToggle);
        assert!(app.labels.passes(&app.sessions[0]));
        assert!(!app.labels.passes(&app.sessions[1]));
        assert_eq!(app.flattened_sessions().len(), 1, "the row list narrows with it");
    }

    #[test]
    fn a_deleted_label_stops_filtering_rather_than_hiding_everything() {
        let mut app = with_labels();
        act(&mut app, LabelAction::OpenFilter);
        act(&mut app, LabelAction::FilterToggle);
        assert_eq!(app.labels.filter.len(), 1);

        act(
            &mut app,
            LabelAction::Loaded(vec![label("l-5", "wave-5", ""), label("l-6", "wave-6", "210")]),
        );
        assert!(app.labels.filter.is_empty(), "the filter is pruned with the catalogue");
        assert_eq!(app.labels.filter_text(), None);
    }

    #[test]
    fn opening_the_filter_with_no_labels_says_so_rather_than_showing_nothing() {
        let mut app = app();
        assert!(act(&mut app, LabelAction::OpenFilter).is_empty());
        assert_eq!(app.view(), View::SessionList);
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn a_narrowing_filter_brings_the_row_cursor_back_inside() {
        let mut app = with_labels();
        act(
            &mut app,
            LabelAction::Attached { session_id: "s-a".to_owned(), label_id: "l-5".to_owned() },
        );
        reduce(&mut app, Action::SelectLast);
        assert_eq!(app.selected_index, 1);
        act(&mut app, LabelAction::OpenFilter);
        act(&mut app, LabelAction::FilterSelectNext);
        act(&mut app, LabelAction::FilterToggle);
        assert_eq!(app.selected_index, 0, "the row it was on is filtered away");
    }
}
