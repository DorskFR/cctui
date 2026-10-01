//! `/` over the session list: a debounced server search with field completion.
//!
//! The grammar is not reimplemented here. `cctui-query` is the crate the server
//! parses `q` with, so the raw query goes over the wire unchanged and this
//! module uses the same parser only to know what to complete and what to
//! highlight — the two can never disagree about what a field is.

use cctui_proto::api::SessionListItem;
use crossterm::event::{KeyCode, KeyEvent};

use super::action::Effect;
use super::state::{App, View};

/// Keystrokes settle for this long before a request goes out.
pub const DEBOUNCE_MS: i64 = 300;

/// Rows per page, and what "load more" asks for again.
pub const LIMIT: i64 = 50;

#[derive(Debug, Clone, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ListSearch {
    pub open: bool,
    pub query: String,
    /// Free-text terms, for highlighting the snippet. Field clauses are the
    /// server's business and are not highlighted.
    pub terms: Vec<String>,
    pub results: Vec<SessionListItem>,
    /// Which result `n`/`N` last landed on.
    pub at: Option<usize>,
    pub include_archived: bool,
    pub has_more: bool,
    pub loading: bool,
    /// Clock reading the pending request is due at; `None` when nothing is
    /// waiting to go out.
    pub due_ms: Option<i64>,
    /// Values offered for the field under the cursor.
    pub values: Vec<String>,
    /// Whether a reply has landed for the text as it stands. Until one has,
    /// the prompt says nothing rather than claiming there is no match.
    pub answered: bool,
    pub error: Option<String>,
}

impl ListSearch {
    #[must_use]
    pub fn is_active(&self) -> bool {
        self.open || !self.query.trim().is_empty()
    }

    /// `12 hits` / `no match`, or nothing before the first reply.
    #[must_use]
    pub fn status(&self) -> Option<String> {
        if self.query.trim().is_empty() {
            return None;
        }
        if let Some(error) = &self.error {
            return Some(error.clone());
        }
        if !self.answered {
            return Some("searching…".to_owned());
        }
        if self.results.is_empty() {
            return Some("no match".to_owned());
        }
        let more = if self.has_more { "+" } else { "" };
        Some(format!("{}{more} hits", self.results.len()))
    }

    fn reset_results(&mut self) {
        self.results.clear();
        self.at = None;
        self.has_more = false;
        self.error = None;
        self.answered = false;
    }
}

/// What `Tab` would complete at the end of the query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Completion {
    /// A bare word that could become a field name.
    Field {
        prefix: String,
    },
    /// `field:` with the value typed so far; the values come from the server.
    Value {
        field: String,
        prefix: String,
    },
    None,
}

/// The last whitespace-separated token, which is the only thing `Tab` acts on.
fn last_token(query: &str) -> &str {
    query.rsplit(' ').next().unwrap_or("")
}

#[must_use]
pub fn completion_target(query: &str) -> Completion {
    let token = last_token(query);
    if token.is_empty() {
        return Completion::None;
    }
    if let Some((field, prefix)) = token.split_once(':') {
        return cctui_query::resolve(field).map_or(Completion::None, |def| Completion::Value {
            field: def.name.to_owned(),
            prefix: prefix.to_owned(),
        });
    }
    Completion::Field { prefix: token.to_owned() }
}

/// Field names the prefix could grow into, in registry order.
#[must_use]
pub fn field_matches(prefix: &str) -> Vec<&'static str> {
    let lower = prefix.to_lowercase();
    cctui_query::FIELDS.iter().filter(|f| f.name.starts_with(&lower)).map(|f| f.name).collect()
}

/// Replaces the last token with `replacement`.
#[must_use]
pub fn apply_completion(query: &str, replacement: &str) -> String {
    let keep = query.len() - last_token(query).len();
    format!("{}{replacement}", &query[..keep])
}

/// Free-text terms of a query, for highlighting. A query that is only field
/// clauses highlights nothing.
#[must_use]
pub fn free_terms(query: &str) -> Vec<String> {
    cctui_query::parse(query).free_text_terms()
}

#[derive(Debug, Clone)]
pub enum ListSearchAction {
    Open,
    Key(KeyEvent),
    Complete,
    Commit,
    Cancel,
    ToggleArchived,
    Next,
    Prev,
    LoadMore,
    Loaded { query: String, offset: usize, sessions: Vec<SessionListItem>, has_more: bool },
    ValuesLoaded { values: Vec<String> },
    Failed(String),
}

pub fn reduce(app: &mut App, action: ListSearchAction) -> Vec<Effect> {
    match action {
        ListSearchAction::Open => {
            if app.view() != View::SessionList {
                return Vec::new();
            }
            app.list_search.open = true;
            Vec::new()
        }
        ListSearchAction::Key(key) => {
            edit(&mut app.list_search, key);
            schedule(app);
            Vec::new()
        }
        ListSearchAction::Complete => complete(app),
        ListSearchAction::Commit => {
            app.list_search.open = false;
            // Enter on a result opens it; on an empty search it just closes.
            open_selected(app)
        }
        ListSearchAction::Cancel => {
            app.list_search = ListSearch::default();
            Vec::new()
        }
        ListSearchAction::ToggleArchived => {
            app.list_search.include_archived = !app.list_search.include_archived;
            app.list_search.reset_results();
            schedule(app);
            Vec::new()
        }
        ListSearchAction::Next => {
            step(app, 1);
            Vec::new()
        }
        ListSearchAction::Prev => {
            step(app, -1);
            Vec::new()
        }
        ListSearchAction::LoadMore => load_more(app),
        ListSearchAction::Loaded { query, offset, sessions, has_more } => {
            // A reply for a query the operator has already moved on from is
            // dropped: the one in flight for the current text will land.
            if query != app.list_search.query {
                return Vec::new();
            }
            app.list_search.loading = false;
            app.list_search.error = None;
            app.list_search.answered = true;
            if offset == 0 {
                app.list_search.results = sessions;
                app.list_search.at = None;
            } else {
                app.list_search.results.extend(sessions);
            }
            app.list_search.has_more = has_more;
            Vec::new()
        }
        ListSearchAction::ValuesLoaded { values } => {
            app.list_search.values = values;
            Vec::new()
        }
        ListSearchAction::Failed(reason) => {
            app.list_search.loading = false;
            app.list_search.answered = true;
            app.list_search.error = Some(reason);
            Vec::new()
        }
    }
}

fn edit(search: &mut ListSearch, key: KeyEvent) {
    match key.code {
        KeyCode::Char(c) => search.query.push(c),
        KeyCode::Backspace => {
            search.query.pop();
        }
        _ => return,
    }
    search.terms = free_terms(&search.query);
    search.values.clear();
    search.answered = false;
}

/// Arms the debounce. The request itself goes out on the tick that finds the
/// deadline passed, so a burst of keystrokes costs one request.
fn schedule(app: &mut App) {
    if app.list_search.query.trim().is_empty() {
        app.list_search.reset_results();
        app.list_search.due_ms = None;
        return;
    }
    app.list_search.due_ms = Some(app.clock_ms + DEBOUNCE_MS);
}

/// Start the list search on `query` without opening the prompt: the command
/// line asked for it, so the results are what the list shows, and the next
/// tick sends the request the same way a keystroke would.
pub fn start_with(app: &mut App, query: String) {
    app.list_search.query = query;
    app.list_search.terms = free_terms(&app.list_search.query);
    app.list_search.reset_results();
    schedule(app);
}

/// Called from the clock tick: fires the pending search once it is due.
pub fn on_tick(app: &mut App) -> Vec<Effect> {
    let Some(due) = app.list_search.due_ms else { return Vec::new() };
    if app.clock_ms < due {
        return Vec::new();
    }
    app.list_search.due_ms = None;
    app.list_search.loading = true;
    vec![Effect::SearchSessions {
        q: app.list_search.query.clone(),
        include_archived: app.list_search.include_archived,
        offset: 0,
    }]
}

fn load_more(app: &mut App) -> Vec<Effect> {
    let search = &mut app.list_search;
    if !search.has_more || search.loading || search.results.is_empty() {
        return Vec::new();
    }
    search.loading = true;
    vec![Effect::SearchSessions {
        q: search.query.clone(),
        include_archived: search.include_archived,
        offset: search.results.len(),
    }]
}

fn complete(app: &mut App) -> Vec<Effect> {
    match completion_target(&app.list_search.query) {
        Completion::Field { prefix } => {
            let matches = field_matches(&prefix);
            let Some(first) = matches.first() else { return Vec::new() };
            // One match completes; several extend to the longest shared prefix
            // so a second Tab can narrow further.
            let completed =
                if matches.len() == 1 { format!("{first}:") } else { common_prefix(&matches) };
            app.list_search.query = apply_completion(&app.list_search.query, &completed);
            app.list_search.terms = free_terms(&app.list_search.query);
            Vec::new()
        }
        Completion::Value { field, prefix } => {
            // Already holding this field's values: complete from them rather
            // than asking again.
            if let Some(first) = app.list_search.values.iter().find(|v| v.starts_with(&prefix)) {
                let token = format!("{field}:{first}");
                app.list_search.query = apply_completion(&app.list_search.query, &token);
                app.list_search.terms = free_terms(&app.list_search.query);
                schedule(app);
                return Vec::new();
            }
            vec![Effect::SearchValues { field, q: prefix }]
        }
        Completion::None => Vec::new(),
    }
}

fn common_prefix(words: &[&str]) -> String {
    let Some(first) = words.first() else { return String::new() };
    let mut len = first.len();
    for word in &words[1..] {
        len = len.min(first.chars().zip(word.chars()).take_while(|(a, b)| a == b).count());
    }
    first.chars().take(len).collect()
}

/// Steps through the results, wrapping. The result list is the list on screen
/// while a search is active, so the selection is the list's own.
fn step(app: &mut App, delta: i32) {
    if app.view() != View::SessionList || app.list_search.results.is_empty() {
        return;
    }
    let len = app.list_search.results.len();
    let next = match app.list_search.at {
        None if delta < 0 => len - 1,
        None => 0,
        Some(at) if delta < 0 => (at + len - 1) % len,
        Some(at) => (at + 1) % len,
    };
    app.list_search.at = Some(next);
    app.selected_index = next;
}

/// Opens the focused result at the seq the match came from, so the transcript
/// lands on the line the snippet showed.
fn open_selected(app: &mut App) -> Vec<Effect> {
    let Some(hit) = focused_result(app) else { return Vec::new() };
    let (id, seq) = (hit.id.clone(), hit.match_seq);
    app.pending_seq_anchor = seq;
    super::conversation::open(app, id)
}

fn focused_result(app: &App) -> Option<&SessionListItem> {
    let search = &app.list_search;
    if search.results.is_empty() {
        return None;
    }
    search.results.get(search.at.unwrap_or(app.selected_index))
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{
        Completion, DEBOUNCE_MS, ListSearchAction, apply_completion, completion_target,
        field_matches, free_terms, on_tick, reduce,
    };
    use crate::app::action::Effect;
    use crate::app::state::App;
    use crate::testsupport::session;

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    fn type_in(app: &mut App, text: &str) {
        for c in text.chars() {
            reduce(app, ListSearchAction::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)));
        }
    }

    fn hit(id: &str, snippet: &str, seq: i64) -> cctui_proto::api::SessionListItem {
        let mut s = session(id, id, "active", "working");
        s.match_snippet = Some(snippet.to_owned());
        s.match_seq = Some(seq);
        s
    }

    #[test]
    fn only_free_text_is_highlighted_not_the_field_clauses() {
        assert_eq!(free_terms("machine:cyberia tag:wave \"auth token\""), ["auth token"]);
        assert_eq!(free_terms("machine:cyberia"), Vec::<String>::new());
        assert_eq!(free_terms("auth token"), ["auth", "token"]);
    }

    #[test]
    fn tab_completes_a_field_name_from_the_shared_registry() {
        assert_eq!(completion_target("mach"), Completion::Field { prefix: "mach".to_owned() });
        assert!(field_matches("mach").contains(&"machine"));
        assert_eq!(apply_completion("tag:x mach", "machine:"), "tag:x machine:");
    }

    #[test]
    fn a_completed_field_switches_tab_to_its_values() {
        assert_eq!(
            completion_target("machine:cy"),
            Completion::Value { field: "machine".to_owned(), prefix: "cy".to_owned() }
        );
        assert_eq!(
            completion_target("m:cy"),
            Completion::Value { field: "machine".to_owned(), prefix: "cy".to_owned() },
            "an alias resolves to its canonical field"
        );
        assert_eq!(completion_target("nonsense:x"), Completion::None);
        assert_eq!(completion_target(""), Completion::None);
    }

    #[test]
    fn one_match_completes_and_several_extend_to_what_they_share() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "machi");
        reduce(&mut app, ListSearchAction::Complete);
        assert_eq!(app.list_search.query, "machine:");

        // `t` is tag, title and tool, which share nothing past the `t` already
        // typed: Tab leaves the token alone rather than picking one.
        app.list_search.query = "t".to_owned();
        reduce(&mut app, ListSearchAction::Complete);
        assert_eq!(app.list_search.query, "t");
        assert!(field_matches("t").len() > 1);
    }

    #[test]
    fn completing_a_value_asks_the_server_then_uses_what_came_back() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "machine:cy");
        match reduce(&mut app, ListSearchAction::Complete).as_slice() {
            [Effect::SearchValues { field, q }] => {
                assert_eq!(field, "machine");
                assert_eq!(q, "cy");
            }
            other => panic!("expected a value fetch, got {} effects", other.len()),
        }

        reduce(&mut app, ListSearchAction::ValuesLoaded { values: vec!["cyberia".to_owned()] });
        assert!(reduce(&mut app, ListSearchAction::Complete).is_empty());
        assert_eq!(app.list_search.query, "machine:cyberia");
    }

    #[test]
    fn a_burst_of_keystrokes_costs_one_request_once_it_settles() {
        let mut app = app();
        app.clock_ms = 1_000;
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "auth");
        assert!(on_tick(&mut app).is_empty(), "nothing goes out while the keys are still coming");

        app.clock_ms = 1_000 + DEBOUNCE_MS - 1;
        assert!(on_tick(&mut app).is_empty());

        app.clock_ms = 1_000 + DEBOUNCE_MS;
        match on_tick(&mut app).as_slice() {
            [Effect::SearchSessions { q, include_archived, offset }] => {
                assert_eq!(q, "auth");
                assert!(!include_archived);
                assert_eq!(*offset, 0);
            }
            other => panic!("expected one search, got {} effects", other.len()),
        }
        assert!(on_tick(&mut app).is_empty(), "and it does not fire twice");
    }

    #[test]
    fn emptying_the_query_drops_the_results_without_a_request() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "a");
        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "a".to_owned(),
                offset: 0,
                sessions: vec![hit("s-x", "match", 4)],
                has_more: false,
            },
        );
        assert_eq!(app.list_search.results.len(), 1);

        reduce(
            &mut app,
            ListSearchAction::Key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE)),
        );
        assert!(app.list_search.results.is_empty());
        assert!(app.list_search.due_ms.is_none());
        assert!(on_tick(&mut app).is_empty());
    }

    #[test]
    fn a_reply_for_an_abandoned_query_is_dropped() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "auth");
        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "aut".to_owned(),
                offset: 0,
                sessions: vec![hit("s-stale", "stale", 1)],
                has_more: false,
            },
        );
        assert!(app.list_search.results.is_empty(), "the text moved on");
    }

    #[test]
    fn the_status_line_counts_the_hits_and_says_when_there_are_none() {
        let mut app = app();
        assert_eq!(app.list_search.status(), None);
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "auth");
        assert_eq!(
            app.list_search.status().as_deref(),
            Some("searching…"),
            "nothing is claimed before the first reply"
        );

        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "auth".to_owned(),
                offset: 0,
                sessions: vec![],
                has_more: false,
            },
        );
        assert_eq!(app.list_search.status().as_deref(), Some("no match"));

        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "auth".to_owned(),
                offset: 0,
                sessions: vec![hit("s-x", "a", 1), hit("s-y", "b", 2)],
                has_more: true,
            },
        );
        assert_eq!(app.list_search.status().as_deref(), Some("2+ hits"));
    }

    #[test]
    fn the_hit_keys_cycle_the_results_and_move_the_selection() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "a");
        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "a".to_owned(),
                offset: 0,
                sessions: vec![hit("s-1", "a", 1), hit("s-2", "b", 2), hit("s-3", "c", 3)],
                has_more: false,
            },
        );

        reduce(&mut app, ListSearchAction::Next);
        assert_eq!(app.list_search.at, Some(0));
        assert_eq!(app.selected_index, 0);
        reduce(&mut app, ListSearchAction::Next);
        assert_eq!(app.list_search.at, Some(1));
        reduce(&mut app, ListSearchAction::Prev);
        assert_eq!(app.list_search.at, Some(0));
        reduce(&mut app, ListSearchAction::Prev);
        assert_eq!(app.list_search.at, Some(2), "stepping back from the first wraps");
    }

    #[test]
    fn enter_opens_the_focused_result_at_the_seq_the_match_came_from() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "a");
        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "a".to_owned(),
                offset: 0,
                sessions: vec![hit("s-1", "a", 11), hit("s-2", "b", 22)],
                has_more: false,
            },
        );
        reduce(&mut app, ListSearchAction::Next);
        reduce(&mut app, ListSearchAction::Next);

        let effects = reduce(&mut app, ListSearchAction::Commit);
        assert_eq!(app.pending_seq_anchor, Some(22));
        assert!(
            effects.iter().any(|e| matches!(e, Effect::LoadConversationPage { session_id, .. } if session_id == "s-2")),
            "the second result is the one that opens"
        );
        assert!(!app.list_search.open);
    }

    #[test]
    fn load_more_asks_from_where_the_results_end() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "a");
        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "a".to_owned(),
                offset: 0,
                sessions: vec![hit("s-1", "a", 1), hit("s-2", "b", 2)],
                has_more: true,
            },
        );
        match reduce(&mut app, ListSearchAction::LoadMore).as_slice() {
            [Effect::SearchSessions { offset, .. }] => assert_eq!(*offset, 2),
            other => panic!("expected one search, got {} effects", other.len()),
        }
        assert!(
            reduce(&mut app, ListSearchAction::LoadMore).is_empty(),
            "not while one is in flight"
        );

        reduce(
            &mut app,
            ListSearchAction::Loaded {
                query: "a".to_owned(),
                offset: 2,
                sessions: vec![hit("s-3", "c", 3)],
                has_more: false,
            },
        );
        assert_eq!(app.list_search.results.len(), 3, "a later page appends");
        assert!(reduce(&mut app, ListSearchAction::LoadMore).is_empty(), "and there is no more");
    }

    #[test]
    fn the_archived_toggle_refetches() {
        let mut app = app();
        app.clock_ms = 500;
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "a");
        reduce(&mut app, ListSearchAction::ToggleArchived);
        assert!(app.list_search.include_archived);
        app.clock_ms += DEBOUNCE_MS;
        match on_tick(&mut app).as_slice() {
            [Effect::SearchSessions { include_archived, .. }] => assert!(*include_archived),
            other => panic!("expected one search, got {} effects", other.len()),
        }
    }

    #[test]
    fn escape_clears_everything() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "auth");
        reduce(&mut app, ListSearchAction::Cancel);
        assert!(!app.list_search.open);
        assert!(app.list_search.query.is_empty());
        assert!(!app.list_search.is_active());
    }

    #[test]
    fn a_failed_search_says_so_instead_of_pretending_there_are_no_hits() {
        let mut app = app();
        reduce(&mut app, ListSearchAction::Open);
        type_in(&mut app, "auth");
        reduce(&mut app, ListSearchAction::Failed("the server is down".to_owned()));
        assert_eq!(app.list_search.status().as_deref(), Some("the server is down"));
        assert!(!app.list_search.loading);
    }

    #[test]
    fn the_prompt_only_opens_over_the_list() {
        let mut app = app();
        app.router.push(crate::app::View::Conversation);
        reduce(&mut app, ListSearchAction::Open);
        assert!(!app.list_search.open);
    }
}
