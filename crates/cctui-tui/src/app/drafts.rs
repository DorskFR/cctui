//! Per-session composer drafts and prompt recall, on the server draft store.

use std::collections::{HashMap, HashSet};

use cctui_clientcore::history_nav::{
    HistoryNav, SESSION_HISTORY_MAX, encode_history, parse_history, push_history,
};
use cctui_proto::drafts::{
    DraftList, PROMPT_HISTORY_KEY, composer_draft_key, draft_session_id, history_session_id,
    session_history_key,
};
use crossterm::event::{KeyCode, KeyEvent};
use ratatui_textarea::CursorMove;

use super::action::Effect;
use super::state::{App, View};

/// How many prompts the recall picker offers, across every session.
const RECENT_MAX: usize = 200;

/// The Ctrl-R recall popup: a filter line over every prompt already sent.
#[derive(Debug, Default)]
pub struct Picker {
    pub filter: String,
    pub selected: usize,
    /// Newest first, de-duped across sessions.
    pub entries: Vec<String>,
}

impl Picker {
    pub fn matches(&self) -> Vec<&str> {
        let needle = self.filter.to_lowercase();
        self.entries
            .iter()
            .filter(|entry| needle.is_empty() || entry.to_lowercase().contains(&needle))
            .map(String::as_str)
            .collect()
    }

    pub fn current(&self) -> Option<&str> {
        self.matches().get(self.selected).copied()
    }
}

/// Composer text and prompt history, keyed by session.
#[derive(Debug, Default)]
pub struct DraftState {
    /// Unsent text per session; the selected session's copy lives in the
    /// text area until the composer moves elsewhere.
    texts: HashMap<String, String>,
    /// Sent prompts per session, most-recent-last.
    history: HashMap<String, Vec<String>>,
    /// Every prompt the recall picker offers, newest first.
    recent: Vec<String>,
    /// Whose draft the text area currently holds.
    pub composer_session: Option<String>,
    /// Set once `GET /drafts` has answered. Until then a session's draft is
    /// simply unknown and no per-session read is worth a request.
    pub index_loaded: bool,
    nav: HistoryNav,
    requested: HashSet<String>,
    pub picker: Option<Picker>,
}

impl DraftState {
    pub fn text(&self, session_id: &str) -> &str {
        self.texts.get(session_id).map_or("", String::as_str)
    }

    pub fn has_draft(&self, session_id: &str) -> bool {
        self.texts.get(session_id).is_some_and(|text| !text.trim().is_empty())
    }

    pub fn history_for(&self, session_id: &str) -> &[String] {
        self.history.get(session_id).map_or(&[], Vec::as_slice)
    }

    fn set_text(&mut self, session_id: String, text: String) {
        if text.is_empty() {
            self.texts.remove(&session_id);
        } else {
            self.texts.insert(session_id, text);
        }
    }

    /// Drop what a session that is no longer listed was holding. The server
    /// rows stay: the session may come back, and the web UI shares them.
    pub fn forget(&mut self, session_id: &str) {
        self.texts.remove(session_id);
        self.history.remove(session_id);
        self.requested.remove(session_id);
        if self.composer_session.as_deref() == Some(session_id) {
            self.composer_session = None;
        }
    }

    /// Adopt `entries` (oldest first) as recallable, newest at the front.
    fn remember(&mut self, entries: &[String]) {
        for entry in entries {
            self.recent.retain(|known| known != entry);
            self.recent.insert(0, entry.clone());
        }
        self.recent.truncate(RECENT_MAX);
    }
}

/// Draft and recall transitions, kept out of the shared reducer.
pub enum DraftAction {
    /// `GET /drafts` answered: every draft this user owns.
    IndexLoaded(Box<DraftList>),
    /// One session's draft and history, re-read as its conversation opened.
    Loaded {
        session_id: String,
        text: Option<String>,
        history: Option<String>,
    },
    HistoryPrev,
    HistoryNext,
    OpenPicker,
    ClosePicker,
    PickerSelectNext,
    PickerSelectPrev,
    PickerRecall,
    PickerKey(KeyEvent),
}

pub fn reduce_drafts(app: &mut App, action: DraftAction) -> Vec<Effect> {
    match action {
        DraftAction::IndexLoaded(list) => {
            index_loaded(app, &list);
            Vec::new()
        }
        DraftAction::Loaded { session_id, text, history } => loaded(app, session_id, text, history),
        DraftAction::HistoryPrev => walk(app, "ArrowUp"),
        DraftAction::HistoryNext => walk(app, "ArrowDown"),
        DraftAction::OpenPicker => open_picker(app),
        DraftAction::ClosePicker => {
            close_picker(app);
            Vec::new()
        }
        DraftAction::PickerSelectNext => {
            move_selection(app, 1);
            Vec::new()
        }
        DraftAction::PickerSelectPrev => {
            move_selection(app, -1);
            Vec::new()
        }
        DraftAction::PickerRecall => recall(app),
        DraftAction::PickerKey(key) => {
            filter_key(app, key);
            Vec::new()
        }
    }
}

fn index_loaded(app: &mut App, list: &DraftList) {
    for draft in &list.drafts {
        if let Some(session_id) = draft_session_id(&draft.key) {
            app.drafts.texts.entry(session_id.to_owned()).or_insert_with(|| draft.text.clone());
        } else if let Some(session_id) = history_session_id(&draft.key) {
            let entries = parse_history(&draft.text);
            app.drafts.remember(&entries);
            app.drafts.history.insert(session_id.to_owned(), entries);
        } else if draft.key == PROMPT_HISTORY_KEY {
            let entries = parse_history(&draft.text);
            app.drafts.remember(&entries);
        }
    }
    app.drafts.index_loaded = true;
    restore_composer(app);
}

/// A freshly read draft never overwrites text the user is already typing: the
/// composer wins over what the server held a moment ago.
fn loaded(
    app: &mut App,
    session_id: String,
    text: Option<String>,
    history: Option<String>,
) -> Vec<Effect> {
    if let Some(raw) = history {
        let entries = parse_history(&raw);
        app.drafts.remember(&entries);
        app.drafts.history.insert(session_id.clone(), entries);
    }
    if let Some(text) = text
        && !app.drafts.has_draft(&session_id)
    {
        app.drafts.set_text(session_id, text);
        restore_composer(app);
    }
    Vec::new()
}

/// Put the composer's own session's draft back in the text area, unless the
/// user is typing into it right now.
fn restore_composer(app: &mut App) {
    if app.input_active {
        return;
    }
    let Some(session_id) = app.drafts.composer_session.clone() else { return };
    let text = app.drafts.text(&session_id).to_owned();
    if text != app.message_input.lines().join("\n") {
        app.set_input_text(&text);
    }
}

/// Keep the text area pointed at the selected session: stash what it holds
/// under the session it was typed for, then restore the new one's draft.
pub fn sync_composer(app: &mut App) -> Vec<Effect> {
    let current = app.selected_session_id();
    if app.drafts.composer_session == current {
        return Vec::new();
    }
    if let Some(previous) = app.drafts.composer_session.take() {
        let text = app.message_input.lines().join("\n");
        app.drafts.set_text(previous, text);
    }
    app.drafts.nav.reset_all();
    app.drafts.composer_session.clone_from(&current);
    let Some(session_id) = current else {
        app.reset_input();
        return Vec::new();
    };
    let text = app.drafts.text(&session_id).to_owned();
    app.set_input_text(&text);
    if app.drafts.index_loaded && app.drafts.requested.insert(session_id.clone()) {
        return vec![Effect::LoadDrafts { session_id }];
    }
    Vec::new()
}

/// Every composer edit: the recall walk is over and the draft needs saving.
pub fn on_input(app: &mut App) -> Vec<Effect> {
    app.drafts.nav.reset();
    if app.drafts.composer_session.is_none() {
        app.drafts.composer_session = app.selected_session_id();
    }
    let Some(session_id) = app.drafts.composer_session.clone() else { return Vec::new() };
    let text = app.message_input.lines().join("\n");
    app.drafts.set_text(session_id.clone(), text.clone());
    vec![Effect::SaveDraft { key: composer_draft_key(&session_id), text }]
}

/// A message left the composer: the draft is spent and its text becomes the
/// newest prompt this session can recall.
pub fn on_send(app: &mut App, session_id: &str, content: &str) -> Vec<Effect> {
    let had_draft = app.drafts.has_draft(session_id);
    app.drafts.texts.remove(session_id);
    app.drafts.nav.reset_all();
    let mut effects = Vec::new();
    if had_draft {
        effects.push(Effect::DiscardDraft { key: composer_draft_key(session_id) });
    }
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return effects;
    }
    app.drafts.remember(&[trimmed.to_owned()]);
    let entries = app.drafts.history.entry(session_id.to_owned()).or_default();
    push_history(entries, content, SESSION_HISTORY_MAX);
    let history = encode_history(entries);
    effects.push(Effect::SaveDraft { key: session_history_key(session_id), text: history });
    effects
}

/// The caret as a character offset, which is what the shared recall rule reads.
fn caret_offset(app: &App) -> usize {
    let cursor = app.message_input.cursor();
    let lines = app.message_input.lines();
    let before: usize = lines.iter().take(cursor.0).map(|line| line.chars().count() + 1).sum();
    before + cursor.1
}

/// An arrow key in the composer: recall when the caret is at the very edge of
/// the text, plain cursor movement otherwise.
fn walk(app: &mut App, key: &str) -> Vec<Effect> {
    let Some(session_id) = app.drafts.composer_session.clone() else { return Vec::new() };
    let entries = app.drafts.history_for(&session_id).to_vec();
    let value = app.message_input.lines().join("\n");
    let caret = caret_offset(app);
    let outcome = app.drafts.nav.handle_key(key, &entries, &value, caret, caret);
    if !outcome.handled {
        let movement = if key == "ArrowUp" { CursorMove::Up } else { CursorMove::Down };
        app.message_input.move_cursor(movement);
        return Vec::new();
    }
    let Some(text) = outcome.value else { return Vec::new() };
    adopt(app, &session_id, text)
}

/// Land recalled text in the composer and keep it as the session's draft.
fn adopt(app: &mut App, session_id: &str, text: String) -> Vec<Effect> {
    app.set_input_text(&text);
    app.input_active = true;
    app.drafts.set_text(session_id.to_owned(), text.clone());
    vec![Effect::SaveDraft { key: composer_draft_key(session_id), text }]
}

fn open_picker(app: &mut App) -> Vec<Effect> {
    let entries = app.drafts.recent.clone();
    app.drafts.picker = Some(Picker { filter: String::new(), selected: 0, entries });
    app.router.push(View::HistoryPicker);
    Vec::new()
}

fn close_picker(app: &mut App) {
    app.drafts.picker = None;
    if app.view() == View::HistoryPicker {
        app.router.pop();
    }
}

fn move_selection(app: &mut App, delta: i32) {
    let Some(picker) = app.drafts.picker.as_mut() else { return };
    let len = picker.matches().len();
    if len == 0 {
        picker.selected = 0;
        return;
    }
    let last = len - 1;
    picker.selected = if delta < 0 {
        picker.selected.checked_sub(1).unwrap_or(last)
    } else if picker.selected >= last {
        0
    } else {
        picker.selected + 1
    };
}

fn recall(app: &mut App) -> Vec<Effect> {
    let Some(pick) = app.drafts.picker.as_ref().and_then(|p| p.current().map(str::to_owned)) else {
        close_picker(app);
        return Vec::new();
    };
    close_picker(app);
    let Some(session_id) = app.drafts.composer_session.clone() else { return Vec::new() };
    let entries = app.drafts.history_for(&session_id).to_vec();
    let value = app.message_input.lines().join("\n");
    let text = app.drafts.nav.recall(&entries, &value, &pick);
    adopt(app, &session_id, text)
}

fn filter_key(app: &mut App, key: KeyEvent) {
    let Some(picker) = app.drafts.picker.as_mut() else { return };
    match key.code {
        KeyCode::Char(c) => picker.filter.push(c),
        KeyCode::Backspace => {
            picker.filter.pop();
        }
        _ => return,
    }
    picker.selected = 0;
}

#[cfg(test)]
mod tests {
    use cctui_proto::drafts::{Draft, DraftList};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{DraftAction, Effect};
    use crate::app::{Action, App, View, reduce};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![
            session("s-a", "alpha", "active", "working"),
            session("s-b", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn draft(key: &str, text: &str) -> Draft {
        Draft {
            key: key.to_owned(),
            text: text.to_owned(),
            updated_at: chrono::DateTime::from_timestamp(0, 0).expect("epoch"),
        }
    }

    fn index(app: &mut App, drafts: Vec<Draft>) -> Vec<Effect> {
        reduce(app, Action::Drafts(DraftAction::IndexLoaded(Box::new(DraftList { drafts }))))
    }

    fn composer(app: &App) -> String {
        app.message_input.lines().join("\n")
    }

    fn type_text(app: &mut App, text: &str) -> Vec<Effect> {
        let mut effects = Vec::new();
        for c in text.chars() {
            effects = if c == '\n' {
                reduce(app, Action::InputNewline)
            } else {
                reduce(app, Action::InputKey(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)))
            };
        }
        effects
    }

    fn saved_draft(effects: &[Effect]) -> Option<(&str, &str)> {
        effects.iter().find_map(|e| match e {
            Effect::SaveDraft { key, text } if key.starts_with("cctui_draft_") => {
                Some((key.as_str(), text.as_str()))
            }
            _ => None,
        })
    }

    fn saved_history(effects: &[Effect]) -> Option<&str> {
        effects.iter().find_map(|e| match e {
            Effect::SaveDraft { key, text } if key.starts_with("cctui_history_") => {
                Some(text.as_str())
            }
            _ => None,
        })
    }

    #[test]
    fn typing_saves_the_draft_under_the_selected_sessions_key() {
        let mut app = app();
        app.input_active = true;
        let effects = type_text(&mut app, "hi");
        assert_eq!(saved_draft(&effects), Some(("cctui_draft_s-a", "hi")));
        assert!(app.drafts.has_draft("s-a"));
    }

    #[test]
    fn a_draft_follows_its_own_session_and_is_invisible_in_another() {
        let mut app = app();
        app.input_active = true;
        type_text(&mut app, "for alpha");
        assert_eq!(composer(&app), "for alpha");

        reduce(&mut app, Action::SelectNext);
        assert_eq!(composer(&app), "", "beta's composer is its own");
        type_text(&mut app, "for beta");

        reduce(&mut app, Action::SelectPrev);
        assert_eq!(composer(&app), "for alpha", "alpha's draft survived the switch");
        reduce(&mut app, Action::SelectNext);
        assert_eq!(composer(&app), "for beta");
    }

    #[test]
    fn a_draft_survives_leaving_and_reopening_the_conversation() {
        let mut app = app();
        app.input_active = true;
        type_text(&mut app, "half a thought");
        reduce(&mut app, Action::LeaveConversation);
        reduce(&mut app, Action::OpenSelectedConversation);
        assert_eq!(composer(&app), "half a thought");
    }

    #[test]
    fn the_server_index_restores_every_session_at_startup() {
        let mut app = app();
        index(
            &mut app,
            vec![
                draft("cctui_draft_s-a", "typed before the restart"),
                draft("cctui_draft_s-b", "beta's turn"),
                draft("cctui_history_s-a", "[\"sent earlier\"]"),
            ],
        );
        assert_eq!(composer(&app), "typed before the restart");
        assert_eq!(app.drafts.text("s-b"), "beta's turn");
        assert_eq!(app.drafts.history_for("s-a"), ["sent earlier".to_owned()]);
    }

    /// A draft read back must never win over what the user is typing now.
    #[test]
    fn the_index_does_not_overwrite_a_live_composer() {
        let mut app = app();
        app.input_active = true;
        type_text(&mut app, "mine");
        index(&mut app, vec![draft("cctui_draft_s-a", "stale copy")]);
        assert_eq!(composer(&app), "mine");
        assert_eq!(app.drafts.text("s-a"), "mine");
    }

    #[test]
    fn a_session_is_re_read_once_the_index_is_known() {
        let mut app = app();
        assert!(
            reduce(&mut app, Action::SelectNext).is_empty(),
            "before the index there is nothing worth fetching"
        );
        index(&mut app, Vec::new());

        let effects = reduce(&mut app, Action::SelectPrev);
        match effects.as_slice() {
            [Effect::LoadDrafts { session_id }] => assert_eq!(session_id, "s-a"),
            _ => panic!("expected one draft read, got {} effects", effects.len()),
        }
        reduce(&mut app, Action::SelectNext);
        assert!(reduce(&mut app, Action::SelectPrev).is_empty(), "read once per run");
    }

    #[test]
    fn a_read_draft_lands_in_the_composer_and_its_history_is_parsed() {
        let mut app = app();
        index(&mut app, Vec::new());
        reduce(
            &mut app,
            Action::Drafts(DraftAction::Loaded {
                session_id: "s-a".to_owned(),
                text: Some("from the web".to_owned()),
                history: Some("[\"older\", \"newer\"]".to_owned()),
            }),
        );
        assert_eq!(composer(&app), "from the web");
        assert_eq!(app.drafts.history_for("s-a"), ["older".to_owned(), "newer".to_owned()]);
    }

    #[test]
    fn sending_clears_the_draft_and_records_the_prompt() {
        let mut app = app();
        app.input_active = true;
        type_text(&mut app, "ship it");
        let effects = reduce(&mut app, Action::SubmitInput);

        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::DiscardDraft { key } if key == "cctui_draft_s-a"
            )),
            "the spent draft is discarded"
        );
        assert_eq!(saved_history(&effects), Some("[\"ship it\"]"));
        assert!(effects.iter().any(|e| matches!(e, Effect::SendMessage { .. })));
        assert!(!app.drafts.has_draft("s-a"));
        assert_eq!(composer(&app), "");
    }

    #[test]
    fn a_blank_submit_records_nothing() {
        let mut app = app();
        app.input_active = true;
        type_text(&mut app, "   ");
        let effects = reduce(&mut app, Action::SubmitInput);
        assert!(effects.is_empty(), "whitespace is neither a message nor a prompt");
        assert!(app.drafts.history_for("s-a").is_empty());
        assert_eq!(app.drafts.text("s-a"), "", "the composer is emptied all the same");
    }

    fn with_history(entries: &[&str]) -> App {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        app.input_active = true;
        for entry in entries {
            type_text(&mut app, entry);
            reduce(&mut app, Action::SubmitInput);
            app.input_active = true;
        }
        app
    }

    #[test]
    fn arrow_up_walks_the_sessions_own_prompts_newest_first() {
        let mut app = with_history(&["first", "second"]);
        reduce(&mut app, Action::Drafts(DraftAction::HistoryPrev));
        assert_eq!(composer(&app), "second");
        reduce(&mut app, Action::Drafts(DraftAction::HistoryPrev));
        assert_eq!(composer(&app), "first");
        reduce(&mut app, Action::Drafts(DraftAction::HistoryPrev));
        assert_eq!(composer(&app), "first", "the walk stops at the oldest");

        reduce(&mut app, Action::Drafts(DraftAction::HistoryNext));
        assert_eq!(composer(&app), "second");
        reduce(&mut app, Action::Drafts(DraftAction::HistoryNext));
        assert_eq!(composer(&app), "", "the live draft comes back");
    }

    #[test]
    fn a_recall_is_saved_as_the_draft_so_a_restart_keeps_it() {
        let mut app = with_history(&["remembered"]);
        let effects = reduce(&mut app, Action::Drafts(DraftAction::HistoryPrev));
        assert_eq!(saved_draft(&effects), Some(("cctui_draft_s-a", "remembered")));
    }

    #[test]
    fn arrow_down_without_a_walk_moves_the_cursor_instead() {
        let mut app = with_history(&["one"]);
        type_text(&mut app, "top\nbottom");
        app.message_input.move_cursor(ratatui_textarea::CursorMove::Top);
        app.message_input.move_cursor(ratatui_textarea::CursorMove::Head);

        reduce(&mut app, Action::Drafts(DraftAction::HistoryNext));
        assert_eq!(composer(&app), "top\nbottom", "the text is untouched");
        assert_eq!(app.message_input.cursor().0, 1, "the caret moved down a line");
    }

    #[test]
    fn a_caret_inside_the_text_scrolls_the_composer_rather_than_recalling() {
        let mut app = with_history(&["one"]);
        type_text(&mut app, "top\nbottom");
        reduce(&mut app, Action::Drafts(DraftAction::HistoryPrev));
        assert_eq!(composer(&app), "top\nbottom");
        assert_eq!(app.message_input.cursor().0, 0);
    }

    #[test]
    fn a_session_with_no_history_recalls_nothing() {
        let mut app = app();
        app.input_active = true;
        type_text(&mut app, "x");
        reduce(&mut app, Action::Drafts(DraftAction::HistoryPrev));
        assert_eq!(composer(&app), "x");
    }

    #[test]
    fn recall_never_reaches_another_sessions_prompts() {
        let mut app = with_history(&["alpha only"]);
        reduce(&mut app, Action::SelectNext);
        reduce(&mut app, Action::Drafts(DraftAction::HistoryPrev));
        assert_eq!(composer(&app), "", "beta has sent nothing");
    }

    #[test]
    fn the_picker_filters_every_sessions_prompts_and_recalls_one() {
        let mut app = with_history(&["deploy the thing", "roll it back"]);
        reduce(&mut app, Action::Drafts(DraftAction::OpenPicker));
        assert_eq!(app.view(), View::HistoryPicker);

        let picker = app.drafts.picker.as_ref().expect("a picker");
        assert_eq!(picker.matches(), vec!["roll it back", "deploy the thing"]);

        for c in "deploy".chars() {
            reduce(
                &mut app,
                Action::Drafts(DraftAction::PickerKey(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::NONE,
                ))),
            );
        }
        let picker = app.drafts.picker.as_ref().expect("a picker");
        assert_eq!(picker.matches(), vec!["deploy the thing"]);

        let effects = reduce(&mut app, Action::Drafts(DraftAction::PickerRecall));
        assert_eq!(app.view(), View::Conversation);
        assert_eq!(composer(&app), "deploy the thing");
        assert_eq!(saved_draft(&effects), Some(("cctui_draft_s-a", "deploy the thing")));
    }

    #[test]
    fn the_picker_offers_prompts_sent_from_another_session() {
        let mut app = with_history(&["alpha's prompt"]);
        reduce(&mut app, Action::SelectNext);
        reduce(&mut app, Action::Drafts(DraftAction::OpenPicker));
        let effects = reduce(&mut app, Action::Drafts(DraftAction::PickerRecall));
        assert_eq!(composer(&app), "alpha's prompt");
        assert_eq!(saved_draft(&effects), Some(("cctui_draft_s-b", "alpha's prompt")));
    }

    #[test]
    fn the_picker_selection_wraps_and_escape_leaves_the_composer_alone() {
        let mut app = with_history(&["one", "two"]);
        type_text(&mut app, "untouched");
        reduce(&mut app, Action::Drafts(DraftAction::OpenPicker));

        reduce(&mut app, Action::Drafts(DraftAction::PickerSelectPrev));
        assert_eq!(app.drafts.picker.as_ref().and_then(super::Picker::current), Some("one"));
        reduce(&mut app, Action::Drafts(DraftAction::PickerSelectNext));
        assert_eq!(app.drafts.picker.as_ref().and_then(super::Picker::current), Some("two"));

        reduce(&mut app, Action::Drafts(DraftAction::ClosePicker));
        assert!(app.drafts.picker.is_none());
        assert_eq!(app.view(), View::Conversation);
        assert_eq!(composer(&app), "untouched");
    }

    #[test]
    fn a_filter_that_matches_nothing_recalls_nothing() {
        let mut app = with_history(&["one"]);
        reduce(&mut app, Action::Drafts(DraftAction::OpenPicker));
        reduce(
            &mut app,
            Action::Drafts(DraftAction::PickerKey(KeyEvent::new(
                KeyCode::Char('z'),
                KeyModifiers::NONE,
            ))),
        );
        assert!(app.drafts.picker.as_ref().expect("a picker").current().is_none());
        reduce(&mut app, Action::Drafts(DraftAction::PickerSelectNext));
        let effects = reduce(&mut app, Action::Drafts(DraftAction::PickerRecall));
        assert_eq!(composer(&app), "");
        assert!(effects.is_empty());
        assert!(app.drafts.picker.is_none());
    }

    #[test]
    fn backspace_widens_the_filter_again() {
        let mut app = with_history(&["one", "two"]);
        reduce(&mut app, Action::Drafts(DraftAction::OpenPicker));
        for code in [KeyCode::Char('o'), KeyCode::Backspace] {
            reduce(
                &mut app,
                Action::Drafts(DraftAction::PickerKey(KeyEvent::new(code, KeyModifiers::NONE))),
            );
        }
        let picker = app.drafts.picker.as_ref().expect("a picker");
        assert!(picker.filter.is_empty());
        assert_eq!(picker.matches().len(), 2);
    }

    #[test]
    fn a_session_that_leaves_the_list_stops_holding_a_draft() {
        let mut app = app();
        app.input_active = true;
        type_text(&mut app, "gone with it");
        reduce(&mut app, Action::SessionDeregistered("s-a".to_owned()));
        assert_eq!(app.drafts.text("s-a"), "");
        assert!(app.drafts.history_for("s-a").is_empty());
        assert_eq!(composer(&app), "", "the next session's own composer is empty");
        assert_eq!(app.drafts.composer_session.as_deref(), Some("s-b"));
    }

    #[test]
    fn only_the_newest_prompts_are_kept_per_session() {
        let sent = ["p1", "p2", "p3", "p4", "p5", "p6"];
        let app = with_history(&sent);
        let history = app.drafts.history_for("s-a");
        assert_eq!(history.len(), 5);
        assert_eq!(history.first().map(String::as_str), Some("p2"));
        assert_eq!(history.last().map(String::as_str), Some("p6"));
    }

    #[test]
    fn a_repeated_prompt_is_not_stored_twice() {
        let app = with_history(&["same", "other", "same"]);
        assert_eq!(app.drafts.history_for("s-a"), ["other".to_owned(), "same".to_owned()]);
    }
}
