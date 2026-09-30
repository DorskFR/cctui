//! The one-line prompt at the bottom of the conversation: `/` searches the
//! transcript, `:` runs a command. Both edit one buffer, so only one can be open.

use crossterm::event::{KeyCode, KeyEvent};

use super::action::Effect;
use super::state::{App, View};
use super::transcript_filter::Filter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Search,
    Command,
}

impl Mode {
    pub const fn sigil(self) -> &'static str {
        match self {
            Self::Search => "/",
            Self::Command => ":",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CmdLine {
    pub open: Option<Mode>,
    pub input: String,
}

impl CmdLine {
    #[must_use]
    pub const fn mode(&self) -> Option<Mode> {
        self.open
    }
}

/// The committed search: the query, its terms and the entries that matched.
///
/// `hits` holds entry indices into the store, so it is recomputed whenever the
/// transcript or the filter moves.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Find {
    pub query: String,
    pub terms: Vec<String>,
    pub hits: Vec<usize>,
    /// Which hit `n`/`N` last landed on.
    pub at: Option<usize>,
}

impl Find {
    #[must_use]
    pub const fn is_active(&self) -> bool {
        !self.terms.is_empty()
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// `3/12` for the status line.
    #[must_use]
    pub fn position(&self) -> Option<String> {
        if !self.is_active() {
            return None;
        }
        if self.hits.is_empty() {
            return Some("no matches".to_owned());
        }
        let at = self.at.map_or(0, |a| a + 1);
        Some(format!("{at}/{}", self.hits.len()))
    }
}

/// A line matches when every term is in it: the server AND-matches terms, and a
/// two-word query that lit up lines containing either word would be useless.
#[must_use]
pub fn line_matches(text: &str, terms: &[String]) -> bool {
    if terms.is_empty() {
        return false;
    }
    let hay = text.to_lowercase();
    terms.iter().all(|t| hay.contains(&t.to_lowercase()))
}

/// Entry indices that match, skipping what the filter is hiding: a hit the
/// reader cannot see is not a hit.
#[must_use]
pub fn hits_in(app: &App, session_id: &str, terms: &[String]) -> Vec<usize> {
    if terms.is_empty() {
        return Vec::new();
    }
    let Some(store) = app.conversations.get(session_id) else { return Vec::new() };
    store
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, entry)| {
            app.filter.visible(&entry.line) && line_matches(&entry.line.text, terms)
        })
        .map(|(index, _)| index)
        .collect()
}

#[derive(Debug, Clone, Copy)]
pub enum CmdAction {
    Open(Mode),
    Key(KeyEvent),
    Commit,
    Cancel,
    NextHit,
    PrevHit,
    /// `f`: step the quick filter.
    CycleFilter,
    /// `F`: the per-category menu.
    ToggleFilterMenu,
    FilterMenuNext,
    FilterMenuPrev,
    FilterMenuToggle,
    FilterShowAll,
    FilterReset,
}

pub fn reduce(app: &mut App, action: CmdAction) -> Vec<Effect> {
    match action {
        CmdAction::Open(mode) => {
            if app.view() != View::Conversation {
                return Vec::new();
            }
            app.cmdline.open = Some(mode);
            app.cmdline.input.clear();
            Vec::new()
        }
        CmdAction::Key(key) => {
            edit(&mut app.cmdline, key);
            // Searching narrows as it is typed; a command only runs on Enter.
            if app.cmdline.open == Some(Mode::Search) {
                let query = app.cmdline.input.clone();
                set_query(app, &query);
            }
            Vec::new()
        }
        CmdAction::Cancel => {
            let was = app.cmdline.open.take();
            app.cmdline.input.clear();
            // Abandoning a search drops its highlight; abandoning a command
            // leaves the transcript alone.
            if was == Some(Mode::Search) {
                app.find.clear();
            }
            Vec::new()
        }
        CmdAction::Commit => commit(app),
        CmdAction::NextHit => {
            step(app, 1);
            Vec::new()
        }
        CmdAction::PrevHit => {
            step(app, -1);
            Vec::new()
        }
        CmdAction::CycleFilter => {
            app.filter.cycle();
            after_filter_change(app)
        }
        CmdAction::ToggleFilterMenu => {
            if app.filter_menu.take().is_some() || app.view() != View::Conversation {
                return Vec::new();
            }
            app.filter_menu = Some(0);
            Vec::new()
        }
        CmdAction::FilterMenuNext => {
            move_menu(app, 1);
            Vec::new()
        }
        CmdAction::FilterMenuPrev => {
            move_menu(app, -1);
            Vec::new()
        }
        CmdAction::FilterMenuToggle => {
            let Some(at) = app.filter_menu else { return Vec::new() };
            let Some(category) = super::transcript_filter::CATEGORIES.get(at).copied() else {
                return Vec::new();
            };
            app.filter.toggle(category);
            after_filter_change(app)
        }
        CmdAction::FilterShowAll => {
            app.filter.show_all();
            after_filter_change(app)
        }
        CmdAction::FilterReset => {
            app.filter.reset();
            after_filter_change(app)
        }
    }
}

fn move_menu(app: &mut App, delta: i32) {
    let len = super::transcript_filter::CATEGORIES.len();
    let at = app.filter_menu.unwrap_or(0);
    let next = if delta < 0 { at.checked_sub(1).unwrap_or(len - 1) } else { (at + 1) % len };
    app.filter_menu = Some(next);
}

/// A filter change moves which rows exist, so the render cache and the hit list
/// both have to be rebuilt, and the new filter is written back to disk.
fn after_filter_change(app: &mut App) -> Vec<Effect> {
    app.filter.quick.as_str().clone_into(&mut app.ui.transcript_quick);
    app.ui.transcript_hidden = app.filter.hidden_names().into_iter().collect();
    if let Some(session_id) = app.selected_session_id() {
        let terms = app.find.terms.clone();
        app.find.hits = hits_in(app, &session_id, &terms);
        app.find.at = None;
        clamp_cursor(app, &session_id);
    }
    vec![Effect::SaveUiState(app.ui.clone())]
}

/// The focused line may have just been filtered away; step to one still shown.
fn clamp_cursor(app: &mut App, session_id: &str) {
    let Some(cursor) = app.line_cursor else { return };
    let Some(store) = app.conversations.get(session_id) else { return };
    let visible: Vec<usize> = store
        .entries()
        .iter()
        .enumerate()
        .filter(|(_, e)| app.filter.visible(&e.line))
        .map(|(i, _)| i)
        .collect();
    if visible.contains(&cursor) {
        return;
    }
    app.line_cursor =
        visible.iter().copied().find(|i| *i > cursor).or_else(|| visible.last().copied());
}

fn edit(cmdline: &mut CmdLine, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => cmdline.input.push(c),
        KeyCode::Backspace => {
            cmdline.input.pop();
        }
        _ => {}
    }
}

fn set_query(app: &mut App, query: &str) {
    query.clone_into(&mut app.find.query);
    app.find.terms = cctui_clientcore::search::tokenize_query(query);
    let Some(session_id) = app.selected_session_id() else { return };
    let terms = app.find.terms.clone();
    app.find.hits = hits_in(app, &session_id, &terms);
    app.find.at = None;
}

fn commit(app: &mut App) -> Vec<Effect> {
    let Some(mode) = app.cmdline.open.take() else { return Vec::new() };
    let input = std::mem::take(&mut app.cmdline.input);
    match mode {
        Mode::Search => {
            set_query(app, &input);
            if app.find.terms.is_empty() {
                app.find.clear();
                return Vec::new();
            }
            // Land on the first hit so Enter is never a no-op.
            step(app, 1);
            Vec::new()
        }
        Mode::Command => super::command::run(app, &input),
    }
}

/// Moves to the next or previous hit and focuses it, so line-select's own scroll
/// and highlight carry the viewport there.
///
/// Decision 7 scopes search to the current view: a diagnose panel or terminal
/// pane over the transcript must not have `n` move a cursor nobody can see.
fn step(app: &mut App, delta: i32) {
    if app.find.hits.is_empty() || app.view() != View::Conversation {
        return;
    }
    let len = app.find.hits.len();
    let next = match app.find.at {
        None if delta < 0 => len - 1,
        None => 0,
        Some(at) if delta < 0 => (at + len - 1) % len,
        Some(at) => (at + 1) % len,
    };
    app.find.at = Some(next);
    app.line_cursor = Some(app.find.hits[next]);
    app.follow_tail = false;
}

/// The filter the TUI starts with: whatever the last run left behind.
#[must_use]
pub fn restore(ui: &crate::config::uistate::UiState) -> Filter {
    let hidden: Vec<String> = ui.transcript_hidden.iter().cloned().collect();
    if ui.transcript_quick.is_empty() && hidden.is_empty() {
        return Filter::default();
    }
    Filter::from_persisted(&ui.transcript_quick, &hidden)
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{CmdAction, Mode, line_matches, reduce};
    use crate::app::state::{App, ConversationLine, LineKind, View};
    use crate::app::transcript_filter::Category;
    use crate::app::{Action, conversation};
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        conversation::open(&mut app, "s-a".to_owned());
        for (seq, kind, text) in [
            (1_i64, LineKind::User, "refactor the parser"),
            (2, LineKind::Thinking { redacted: false }, "two parsers to weigh"),
            (3, LineKind::Tool { category: crate::app::ToolCategory::Read }, "src/parser.rs"),
            (4, LineKind::Result { error: false }, "120 lines"),
            (5, LineKind::Assistant, "the parser is done"),
        ] {
            app.conversation_mut("s-a").push_live(Some(seq), ConversationLine::new(kind, text, 0));
        }
        app
    }

    fn key(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    fn type_in(app: &mut App, text: &str) {
        for c in text.chars() {
            reduce(app, CmdAction::Key(key(c)));
        }
    }

    #[test]
    fn every_term_has_to_be_present_for_a_line_to_match() {
        let terms = vec!["parse".to_owned(), "done".to_owned()];
        assert!(line_matches("the parser is done", &terms));
        assert!(!line_matches("the parser is running", &terms));
        assert!(!line_matches("anything", &[]), "an empty query matches nothing");
        assert!(line_matches("PARSER", &["parser".to_owned()]), "matching is case-insensitive");
    }

    #[test]
    fn a_search_narrows_as_it_is_typed_and_lands_on_the_first_hit() {
        let mut app = app();
        reduce(&mut app, CmdAction::Open(Mode::Search));
        assert_eq!(app.cmdline.mode(), Some(Mode::Search));

        type_in(&mut app, "parser");
        assert_eq!(app.find.hits, [0, 1, 2, 4]);
        assert_eq!(app.find.at, None, "typing highlights but does not jump");

        reduce(&mut app, CmdAction::Commit);
        assert!(app.cmdline.open.is_none(), "committing closes the prompt");
        assert_eq!(app.find.at, Some(0));
        assert_eq!(app.line_cursor, Some(0), "the hit is focused so the view scrolls to it");
        assert_eq!(app.find.position().as_deref(), Some("1/4"));
    }

    #[test]
    fn the_hit_keys_wrap_in_both_directions() {
        let mut app = app();
        reduce(&mut app, CmdAction::Open(Mode::Search));
        type_in(&mut app, "parser");
        reduce(&mut app, CmdAction::Commit);

        reduce(&mut app, CmdAction::NextHit);
        assert_eq!(app.line_cursor, Some(1));
        reduce(&mut app, CmdAction::PrevHit);
        assert_eq!(app.line_cursor, Some(0));
        reduce(&mut app, CmdAction::PrevHit);
        assert_eq!(app.line_cursor, Some(4), "stepping back from the first wraps to the last");
        reduce(&mut app, CmdAction::NextHit);
        assert_eq!(app.line_cursor, Some(0));
    }

    #[test]
    fn backspace_edits_the_query_and_escape_drops_the_search() {
        let mut app = app();
        reduce(&mut app, CmdAction::Open(Mode::Search));
        type_in(&mut app, "parserx");
        assert!(app.find.hits.is_empty());
        assert_eq!(app.find.position().as_deref(), Some("no matches"));

        reduce(&mut app, CmdAction::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)));
        assert_eq!(app.cmdline.input, "parser");
        assert_eq!(app.find.hits, [0, 1, 2, 4]);

        reduce(&mut app, CmdAction::Cancel);
        assert!(app.cmdline.open.is_none());
        assert!(!app.find.is_active(), "the highlight goes with the prompt");
    }

    #[test]
    fn a_quoted_phrase_stays_one_term() {
        let mut app = app();
        reduce(&mut app, CmdAction::Open(Mode::Search));
        type_in(&mut app, "\"the parser\"");
        assert_eq!(app.find.terms, ["the parser"]);
        assert_eq!(app.find.hits, [0, 4], "the two lines carrying the phrase, not the words apart");
    }

    #[test]
    fn the_hit_keys_are_inert_outside_the_conversation() {
        let mut app = app();
        reduce(&mut app, CmdAction::Open(Mode::Search));
        type_in(&mut app, "parser");
        reduce(&mut app, CmdAction::Commit);
        let landed = app.line_cursor;

        app.router.push(View::Diagnose);
        reduce(&mut app, CmdAction::NextHit);
        assert_eq!(app.line_cursor, landed, "a panel over the transcript swallows n");
        reduce(&mut app, CmdAction::Open(Mode::Search));
        assert!(app.cmdline.open.is_none(), "and it cannot open the prompt either");
    }

    #[test]
    fn stepping_hits_with_no_search_does_nothing() {
        let mut app = app();
        reduce(&mut app, CmdAction::NextHit);
        assert!(app.line_cursor.is_none());
        assert!(app.find.position().is_none());
    }

    #[test]
    fn a_filtered_away_line_is_not_a_hit() {
        let mut app = app();
        reduce(&mut app, CmdAction::Open(Mode::Search));
        type_in(&mut app, "parser");
        reduce(&mut app, CmdAction::Commit);
        assert_eq!(app.find.hits, [0, 1, 2, 4]);

        app.filter.toggle(Category::Thinking);
        let effects = super::after_filter_change(&mut app);
        assert!(!effects.is_empty(), "the filter is persisted");
        assert_eq!(app.find.hits, [0, 2, 4], "the hidden thinking line drops out");
    }

    #[test]
    fn the_quick_filter_cycles_and_persists() {
        let mut app = app();
        assert_eq!(reduce(&mut app, CmdAction::CycleFilter).len(), 1);
        assert_eq!(app.filter.quick.as_str(), "assistant");
        assert_eq!(app.ui.transcript_quick, "assistant");
        let restored = super::restore(&app.ui);
        assert_eq!(restored, app.filter);
    }

    #[test]
    fn the_menu_wraps_and_toggles_the_focused_category() {
        let mut app = app();
        reduce(&mut app, CmdAction::ToggleFilterMenu);
        assert_eq!(app.filter_menu, Some(0));
        reduce(&mut app, CmdAction::FilterMenuPrev);
        assert_eq!(
            app.filter_menu,
            Some(crate::app::transcript_filter::CATEGORIES.len() - 1),
            "stepping back from the top wraps"
        );
        reduce(&mut app, CmdAction::FilterMenuNext);
        assert_eq!(app.filter_menu, Some(0));

        reduce(&mut app, CmdAction::FilterMenuToggle);
        assert!(!app.filter.shows(Category::Assistant));
        reduce(&mut app, CmdAction::ToggleFilterMenu);
        assert!(app.filter_menu.is_none(), "the same key closes it");
    }

    #[test]
    fn hiding_the_focused_line_moves_the_cursor_to_one_still_shown() {
        let mut app = app();
        app.line_cursor = Some(1);
        app.filter.toggle(Category::Thinking);
        super::after_filter_change(&mut app);
        assert_eq!(app.line_cursor, Some(2), "the next visible line takes the focus");
    }

    #[test]
    fn the_prompt_only_opens_over_the_conversation() {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        assert_eq!(app.view(), View::SessionList);
        reduce(&mut app, CmdAction::Open(Mode::Search));
        assert!(app.cmdline.open.is_none());
        reduce(&mut app, CmdAction::ToggleFilterMenu);
        assert!(app.filter_menu.is_none());
    }

    #[test]
    fn show_all_and_reset_are_both_reachable_and_persisted() {
        let mut app = app();
        reduce(&mut app, CmdAction::FilterShowAll);
        assert!(app.filter.shows(Category::Mcp));
        assert!(app.ui.transcript_hidden.is_empty());

        reduce(&mut app, CmdAction::FilterReset);
        assert!(!app.filter.shows(Category::Mcp));
        assert_eq!(app.filter, crate::app::transcript_filter::Filter::default());
    }

    #[test]
    fn the_search_action_reaches_the_reducer_through_the_action_enum() {
        let mut app = app();
        crate::app::reduce(&mut app, Action::CmdLine(CmdAction::Open(Mode::Search)));
        assert_eq!(app.cmdline.mode(), Some(Mode::Search));
    }
}
