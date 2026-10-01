//! The saved-message collection: browse, search, open at the source, copy,
//! edit and prune.

use cctui_clientcore::bookmarks::{
    bookmark_markdown, default_title, is_dead_link, query_terms, snapshot_body,
};
use cctui_proto::api::bookmarks::{Bookmark, CreateBookmark};
use crossterm::event::{KeyCode, KeyEvent};
use uuid::Uuid;

use super::action::Effect;
use super::state::{App, LineKind};
use super::toast::Level;

/// Rows per page; the server clamps anything larger.
pub const PAGE: i64 = 50;

/// Which field an edit prompt is typing into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Title,
    Note,
}

/// The one-line prompt the view can open over itself.
#[derive(Debug)]
pub enum Prompt {
    Search {
        buffer: String,
    },
    Edit {
        id: Uuid,
        field: Field,
        title: String,
        note: String,
    },
    /// Saving a transcript message: the draft carries the snapshot, the prompt
    /// only edits its title and note.
    Save {
        draft: Box<CreateBookmark>,
        field: Field,
    },
}

impl Prompt {
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Search { .. } => "search",
            Self::Edit { field: Field::Title, .. } | Self::Save { field: Field::Title, .. } => {
                "title"
            }
            Self::Edit { field: Field::Note, .. } | Self::Save { field: Field::Note, .. } => "note",
        }
    }

    pub fn buffer(&self) -> &str {
        match self {
            Self::Search { buffer } => buffer,
            Self::Edit { field: Field::Title, title, .. } => title,
            Self::Edit { field: Field::Note, note, .. } => note,
            Self::Save { draft, field: Field::Title } => &draft.title,
            Self::Save { draft, field: Field::Note } => draft.note.as_deref().unwrap_or(""),
        }
    }

    fn buffer_mut(&mut self) -> &mut String {
        match self {
            Self::Search { buffer } => buffer,
            Self::Edit { field: Field::Title, title, .. } => title,
            Self::Edit { field: Field::Note, note, .. } => note,
            Self::Save { draft, field: Field::Title } => &mut draft.title,
            Self::Save { draft, field: Field::Note } => draft.note.get_or_insert_with(String::new),
        }
    }
}

#[derive(Debug, Default)]
pub struct BookmarkState {
    /// Loaded rows, newest first, in server order.
    pub rows: Vec<Bookmark>,
    /// Index into [`Self::visible`].
    pub selected: usize,
    /// The query the server filtered the loaded rows on.
    pub query: String,
    pub prompt: Option<Prompt>,
    /// The bookmark a delete is waiting to be confirmed for.
    pub confirm: Option<Uuid>,
    pub loading: bool,
    /// The server returned a short page: there is nothing older.
    pub exhausted: bool,
    pub loaded_once: bool,
    pub preview_scroll: usize,
}

impl BookmarkState {
    /// A search prompt narrows the loaded rows as it is typed; committing it
    /// re-asks the server, which can see rows this page does not hold.
    pub fn visible(&self) -> Vec<&Bookmark> {
        let terms = self.live_terms();
        if terms.is_empty() {
            return self.rows.iter().collect();
        }
        self.rows.iter().filter(|b| terms.iter().all(|t| matches_term(b, t))).collect()
    }

    /// Terms to highlight: what is being typed, else what the server matched.
    pub fn terms(&self) -> Vec<String> {
        let live = self.live_terms();
        if live.is_empty() { query_terms(&self.query) } else { live }
    }

    fn live_terms(&self) -> Vec<String> {
        match self.prompt.as_ref() {
            Some(Prompt::Search { buffer }) => query_terms(buffer),
            _ => Vec::new(),
        }
    }

    pub fn selected_bookmark(&self) -> Option<&Bookmark> {
        self.visible().get(self.selected).copied()
    }

    /// How many rows the preview can scroll through: the heading, the note and
    /// the body. Markdown wrapping only ever makes it longer, so this is the
    /// floor that stops the scroll running past the end.
    pub fn preview_rows(&self) -> usize {
        self.selected_bookmark().map_or(0, |b| {
            b.body.lines().count()
                + 2
                + usize::from(b.note.as_deref().is_some_and(|n| !n.is_empty()))
        })
    }

    fn clamp(&mut self) {
        let len = self.visible().len();
        self.selected = if len == 0 { 0 } else { self.selected.min(len - 1) };
    }
}

fn matches_term(b: &Bookmark, term: &str) -> bool {
    let needle = term.to_lowercase();
    b.title.to_lowercase().contains(&needle)
        || b.body.to_lowercase().contains(&needle)
        || b.note.as_deref().is_some_and(|n| n.to_lowercase().contains(&needle))
}

/// `age` of a bookmark, in the shared wording the web UI uses.
#[must_use]
pub fn age_label(created_at: chrono::DateTime<chrono::Utc>, now_ms: i64) -> String {
    let secs = (now_ms - created_at.timestamp_millis()).max(0) / 1_000;
    cctui_clientcore::format::uptime(u64::try_from(secs).unwrap_or(0))
}

pub enum BookmarkAction {
    Loaded {
        rows: Vec<Bookmark>,
        append: bool,
    },
    Failed,
    SelectNext,
    SelectPrev,
    SelectFirst,
    SelectLast,
    PreviewDown,
    PreviewUp,
    SearchOpen,
    EditOpen,
    PromptKey(KeyEvent),
    PromptSwitch,
    PromptCommit,
    PromptCancel,
    DeleteAsk,
    DeleteConfirm,
    DeleteCancel,
    Deleted {
        id: Uuid,
    },
    Updated(Box<Bookmark>),
    OpenSource,
    CopyMarkdown,
    /// `b` in line-select: open the save form for the focused message.
    Save(KeyEvent),
    Saved(Box<Bookmark>),
}

pub fn reduce_bookmarks(app: &mut App, action: BookmarkAction) -> Vec<Effect> {
    match action {
        BookmarkAction::Loaded { rows, append } => {
            loaded(app, rows, append);
            Vec::new()
        }
        BookmarkAction::Failed => {
            app.bookmarks.loading = false;
            app.toast(Level::Warn, "could not load the bookmarks");
            Vec::new()
        }
        BookmarkAction::SelectNext => select(app, 1),
        BookmarkAction::SelectPrev => select(app, -1),
        BookmarkAction::SelectFirst => {
            app.bookmarks.selected = 0;
            app.bookmarks.preview_scroll = 0;
            Vec::new()
        }
        BookmarkAction::SelectLast => {
            let len = app.bookmarks.visible().len();
            app.bookmarks.selected = len.saturating_sub(1);
            app.bookmarks.preview_scroll = 0;
            Vec::new()
        }
        BookmarkAction::PreviewDown => {
            let last = app.bookmarks.preview_rows().saturating_sub(1);
            app.bookmarks.preview_scroll = (app.bookmarks.preview_scroll + 1).min(last);
            Vec::new()
        }
        BookmarkAction::PreviewUp => {
            app.bookmarks.preview_scroll = app.bookmarks.preview_scroll.saturating_sub(1);
            Vec::new()
        }
        BookmarkAction::SearchOpen => {
            app.bookmarks.prompt = Some(Prompt::Search { buffer: app.bookmarks.query.clone() });
            Vec::new()
        }
        BookmarkAction::EditOpen => {
            edit_open(app);
            Vec::new()
        }
        BookmarkAction::PromptKey(key) => {
            prompt_key(app, key);
            Vec::new()
        }
        BookmarkAction::PromptSwitch => {
            match app.bookmarks.prompt.as_mut() {
                Some(Prompt::Edit { field, .. } | Prompt::Save { field, .. }) => {
                    *field = if *field == Field::Title { Field::Note } else { Field::Title };
                }
                _ => return Vec::new(),
            }
            Vec::new()
        }
        BookmarkAction::PromptCommit => prompt_commit(app),
        BookmarkAction::PromptCancel => {
            app.bookmarks.prompt = None;
            app.bookmarks.clamp();
            Vec::new()
        }
        BookmarkAction::DeleteAsk => {
            app.bookmarks.confirm = app.bookmarks.selected_bookmark().map(|b| b.id);
            Vec::new()
        }
        BookmarkAction::DeleteConfirm => {
            let Some(id) = app.bookmarks.confirm.take() else { return Vec::new() };
            vec![Effect::DeleteBookmark { id: id.to_string() }]
        }
        BookmarkAction::DeleteCancel => {
            app.bookmarks.confirm = None;
            Vec::new()
        }
        BookmarkAction::Deleted { id } => {
            app.bookmarks.rows.retain(|b| b.id != id);
            app.bookmarks.clamp();
            app.toast(Level::Info, "deleted the bookmark");
            Vec::new()
        }
        BookmarkAction::Updated(bookmark) => {
            if let Some(row) = app.bookmarks.rows.iter_mut().find(|b| b.id == bookmark.id) {
                *row = *bookmark;
            }
            app.toast(Level::Info, "saved the bookmark");
            Vec::new()
        }
        BookmarkAction::OpenSource => open_source(app),
        BookmarkAction::CopyMarkdown => copy(app),
        BookmarkAction::Save(key) => save_focused(app, key),
        BookmarkAction::Saved(bookmark) => {
            app.toast(Level::Info, format!("saved “{}”", bookmark.title));
            // The list is read per visit, so a save during this one has to land
            // in it rather than wait for the next.
            if app.bookmarks.loaded_once {
                app.bookmarks.rows.insert(0, *bookmark);
                app.bookmarks.clamp();
            }
            Vec::new()
        }
    }
}

/// The role names the web UI's line model uses, so a bookmark saved here reads
/// the same in both clients.
const fn role_of(kind: LineKind) -> &'static str {
    match kind {
        LineKind::User => "user",
        LineKind::Assistant | LineKind::Image => "assistant",
        LineKind::Thinking { .. } => "thinking",
        LineKind::Tool { .. } => "tool",
        LineKind::Result { .. } => "result",
        LineKind::Peer => "peer",
        LineKind::Marker => "marker",
        LineKind::Reset => "reset",
        LineKind::Compact => "compact",
        LineKind::Summary => "summary",
        LineKind::System | LineKind::Reply => "system",
    }
}

fn is_mcp(tool: Option<&str>) -> bool {
    tool.is_some_and(|t| t.starts_with("mcp__"))
}

/// Save the line under the transcript cursor. The body is snapshotted the way
/// the web UI snapshots it, and the title is its first line.
fn save_focused(app: &mut App, key: KeyEvent) -> Vec<Effect> {
    let Some(cursor) = app.line_cursor else {
        return super::reduce(app, super::action::Action::ActivateInputWith(key));
    };
    let Some(session_id) = app.selected_session_id() else { return Vec::new() };
    let session_name =
        app.sessions.iter().find(|s| s.id == session_id).and_then(|s| s.name.clone());
    let Some(entry) = app.conversation_mut(&session_id).entries().get(cursor) else {
        return Vec::new();
    };
    let role = role_of(entry.line.kind);
    let tool = entry.line.tool.as_deref();
    let body = snapshot_body(role, &entry.line.text, tool, is_mcp(tool), None);
    let draft = CreateBookmark {
        session_id: Some(session_id),
        seq: entry.sequenced.then_some(entry.seq),
        message_id: entry.line.message_id.clone(),
        title: default_title(&body),
        body,
        role: role.to_owned(),
        session_name,
        note: None,
        message_ts: entry.line.timestamp.saturating_mul(1_000),
    };
    app.bookmarks.prompt = Some(Prompt::Save { draft: Box::new(draft), field: Field::Title });
    Vec::new()
}

/// Entering the slice reads the first page once; the switcher owns the routing.
pub fn on_enter(app: &mut App) -> Vec<Effect> {
    if app.bookmarks.loaded_once || app.bookmarks.loading {
        return Vec::new();
    }
    app.bookmarks.loading = true;
    vec![Effect::LoadBookmarks { q: app.bookmarks.query.clone(), before: None }]
}

fn loaded(app: &mut App, rows: Vec<Bookmark>, append: bool) {
    let short_page = i64::try_from(rows.len()).unwrap_or(i64::MAX) < PAGE;
    app.bookmarks.loading = false;
    app.bookmarks.loaded_once = true;
    app.bookmarks.exhausted = short_page;
    if append {
        let known: std::collections::HashSet<Uuid> =
            app.bookmarks.rows.iter().map(|b| b.id).collect();
        app.bookmarks.rows.extend(rows.into_iter().filter(|b| !known.contains(&b.id)));
    } else {
        app.bookmarks.rows = rows;
        app.bookmarks.selected = 0;
        app.bookmarks.preview_scroll = 0;
    }
    app.bookmarks.clamp();
}

/// Walking past the last loaded row asks for the next page, as scrolling the
/// web list does.
fn select(app: &mut App, delta: i32) -> Vec<Effect> {
    let len = app.bookmarks.visible().len();
    if len == 0 {
        return Vec::new();
    }
    app.bookmarks.preview_scroll = 0;
    if delta < 0 {
        app.bookmarks.selected = app.bookmarks.selected.saturating_sub(1);
        return Vec::new();
    }
    let at_end = app.bookmarks.selected + 1 >= len;
    if !at_end {
        app.bookmarks.selected += 1;
        return Vec::new();
    }
    load_more(app)
}

fn load_more(app: &mut App) -> Vec<Effect> {
    if app.bookmarks.exhausted || app.bookmarks.loading {
        return Vec::new();
    }
    let Some(before) = app.bookmarks.rows.last().map(|b| b.created_at) else { return Vec::new() };
    app.bookmarks.loading = true;
    vec![Effect::LoadBookmarks { q: app.bookmarks.query.clone(), before: Some(before) }]
}

fn edit_open(app: &mut App) {
    let Some(b) = app.bookmarks.selected_bookmark() else { return };
    app.bookmarks.prompt = Some(Prompt::Edit {
        id: b.id,
        field: Field::Title,
        title: b.title.clone(),
        note: b.note.clone().unwrap_or_default(),
    });
}

fn prompt_key(app: &mut App, key: KeyEvent) {
    let Some(prompt) = app.bookmarks.prompt.as_mut() else { return };
    match key.code {
        KeyCode::Char(c) => prompt.buffer_mut().push(c),
        KeyCode::Backspace => {
            prompt.buffer_mut().pop();
        }
        _ => return,
    }
    if matches!(prompt, Prompt::Search { .. }) {
        app.bookmarks.selected = 0;
        app.bookmarks.clamp();
    }
}

fn prompt_commit(app: &mut App) -> Vec<Effect> {
    let Some(prompt) = app.bookmarks.prompt.take() else { return Vec::new() };
    match prompt {
        Prompt::Search { buffer } => {
            app.bookmarks.query = buffer;
            app.bookmarks.rows.clear();
            app.bookmarks.selected = 0;
            app.bookmarks.exhausted = false;
            app.bookmarks.loading = true;
            vec![Effect::LoadBookmarks { q: app.bookmarks.query.clone(), before: None }]
        }
        Prompt::Save { mut draft, .. } => {
            let title = draft.title.trim().to_owned();
            draft.title = title;
            if draft.title.is_empty() {
                app.toast(Level::Warn, "a bookmark needs a title");
                return Vec::new();
            }
            draft.note = draft.note.map(|n| n.trim().to_owned()).filter(|n| !n.is_empty());
            vec![Effect::CreateBookmark { draft }]
        }
        Prompt::Edit { id, title, note, .. } => {
            let title = title.trim().to_owned();
            if title.is_empty() {
                app.toast(Level::Warn, "a bookmark needs a title");
                return Vec::new();
            }
            let note = note.trim().to_owned();
            vec![Effect::UpdateBookmark {
                id: id.to_string(),
                title,
                note: (!note.is_empty()).then_some(note),
            }]
        }
    }
}

/// Open the source message: the session the bookmark came from, scrolled to the
/// seq it was saved at. A bookmark outlives its session, so both the dead link
/// and a session the list no longer holds have to say so.
fn open_source(app: &mut App) -> Vec<Effect> {
    let Some(b) = app.bookmarks.selected_bookmark() else { return Vec::new() };
    if is_dead_link(b.session_id.as_deref()) {
        app.toast(Level::Warn, "source deleted");
        return Vec::new();
    }
    let (Some(session_id), seq) = (b.session_id.clone(), b.seq) else { return Vec::new() };
    if !app.flattened_sessions().iter().any(|s| s.id == session_id) {
        app.toast(Level::Warn, "source session is no longer listed");
        return Vec::new();
    }
    app.bookmarks.prompt = None;
    app.bookmarks.confirm = None;
    // The source lives in the sessions slice, so the move is a slice change,
    // not a push: the tab bar has to agree with what is on screen.
    app.slice = super::slice::Slice::Sessions;
    app.router.reset(super::slice::Slice::Sessions.root());
    let effects = super::conversation::switch_to(app, session_id);
    if let Some(seq) = seq {
        super::pins::arm_jump(app, seq);
    }
    effects
}

fn copy(app: &App) -> Vec<Effect> {
    let Some(b) = app.bookmarks.selected_bookmark() else { return Vec::new() };
    let text = bookmark_markdown(&b.title, b.note.as_deref(), &b.body);
    vec![Effect::Copy { text, label: "bookmark" }]
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, TimeZone, Utc};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use uuid::Uuid;

    use super::{BookmarkAction, Field, PAGE, Prompt, age_label};
    use crate::app::action::Effect;
    use crate::app::state::{App, View};
    use crate::app::{Action, reduce};
    use crate::testsupport::session;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).single().expect("a timestamp")
    }

    fn bookmark(
        title: &str,
        session: Option<&str>,
        seq: Option<i64>,
    ) -> cctui_proto::api::bookmarks::Bookmark {
        cctui_proto::api::bookmarks::Bookmark {
            id: Uuid::new_v4(),
            session_id: session.map(str::to_owned),
            seq,
            message_id: None,
            title: title.to_owned(),
            body: format!("the body of {title}"),
            role: "assistant".to_owned(),
            session_name: session.map(|_| "fix-auth".to_owned()),
            note: Some(format!("note on {title}")),
            message_ts: at(1_000),
            created_at: at(2_000),
        }
    }

    fn app() -> App {
        let mut app = App::new();
        app.sessions = vec![session("s-a", "alpha", "active", "working")];
        app.update_aggregates();
        app
    }

    /// The tab the switcher lands on, which is also what reads the first page.
    fn enter(app: &mut App) -> Vec<Effect> {
        reduce(app, Action::Slice(crate::app::slice::SliceAction::Switch(2)))
    }

    fn with_rows(rows: Vec<cctui_proto::api::bookmarks::Bookmark>) -> App {
        let mut app = app();
        enter(&mut app);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Loaded { rows, append: false }));
        app
    }

    fn type_text(app: &mut App, text: &str) {
        for c in text.chars() {
            reduce(
                app,
                Action::Bookmarks(BookmarkAction::PromptKey(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::NONE,
                ))),
            );
        }
    }

    fn titles(app: &App) -> Vec<String> {
        app.bookmarks.visible().iter().map(|b| b.title.clone()).collect()
    }

    #[test]
    fn landing_on_the_tab_reads_the_first_page_once() {
        let mut app = app();
        let effects = enter(&mut app);
        assert_eq!(app.view(), View::Bookmarks);
        assert_eq!(app.slice, crate::app::slice::Slice::Bookmarks);
        match effects.as_slice() {
            [Effect::LoadBookmarks { q, before: None }] => assert!(q.is_empty()),
            _ => panic!("expected the first page to be read"),
        }

        reduce(
            &mut app,
            Action::Bookmarks(BookmarkAction::Loaded {
                rows: vec![bookmark("one", Some("s-a"), Some(4))],
                append: false,
            }),
        );
        reduce(&mut app, Action::Slice(crate::app::slice::SliceAction::Switch(1)));
        assert_eq!(app.view(), View::SessionList);
        assert!(enter(&mut app).is_empty(), "coming back does not refetch what is already loaded");
    }

    #[test]
    fn typing_a_search_narrows_the_loaded_rows_and_committing_asks_the_server() {
        let mut app = with_rows(vec![
            bookmark("Gateway fix summary", Some("s-a"), Some(4)),
            bookmark("Old plan", None, None),
        ]);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::SearchOpen));
        type_text(&mut app, "gateway");
        assert_eq!(titles(&app), vec!["Gateway fix summary".to_owned()], "narrowed as typed");
        assert_eq!(app.bookmarks.terms(), vec!["gateway".to_owned()], "and highlighted");

        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCommit));
        match effects.as_slice() {
            [Effect::LoadBookmarks { q, before: None }] => assert_eq!(q, "gateway"),
            _ => panic!("expected a server search"),
        }
        assert!(app.bookmarks.rows.is_empty(), "the old page is dropped");
        assert!(app.bookmarks.prompt.is_none());
    }

    #[test]
    fn a_search_that_matches_nothing_leaves_the_selection_in_range() {
        let mut app = with_rows(vec![bookmark("one", Some("s-a"), None)]);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::SearchOpen));
        type_text(&mut app, "zzz");
        assert!(titles(&app).is_empty());
        assert_eq!(app.bookmarks.selected, 0);
        assert!(app.bookmarks.selected_bookmark().is_none());
        assert!(reduce(&mut app, Action::Bookmarks(BookmarkAction::OpenSource)).is_empty());
    }

    #[test]
    fn escaping_a_search_keeps_the_rows() {
        let mut app = with_rows(vec![bookmark("one", Some("s-a"), None)]);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::SearchOpen));
        type_text(&mut app, "zzz");
        reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCancel));
        assert_eq!(titles(&app), vec!["one".to_owned()]);
        assert!(app.bookmarks.terms().is_empty());
    }

    #[test]
    fn walking_past_the_last_row_pages_on_the_oldest_created_at() {
        let rows: Vec<_> =
            (0..PAGE).map(|i| bookmark(&format!("b{i}"), Some("s-a"), None)).collect();
        let mut app = with_rows(rows);
        assert!(!app.bookmarks.exhausted, "a full page means there may be more");

        for _ in 0..PAGE - 1 {
            reduce(&mut app, Action::Bookmarks(BookmarkAction::SelectNext));
        }
        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::SelectNext));
        match effects.as_slice() {
            [Effect::LoadBookmarks { before: Some(before), .. }] => {
                assert_eq!(*before, at(2_000));
            }
            _ => panic!("expected the next page"),
        }
        assert!(
            reduce(&mut app, Action::Bookmarks(BookmarkAction::SelectNext)).is_empty(),
            "one page in flight at a time"
        );

        reduce(
            &mut app,
            Action::Bookmarks(BookmarkAction::Loaded {
                rows: vec![bookmark("older", Some("s-a"), None)],
                append: true,
            }),
        );
        assert_eq!(i64::try_from(app.bookmarks.rows.len()).unwrap_or(0), PAGE + 1);
        assert!(app.bookmarks.exhausted, "a short page is the end");
        assert!(reduce(&mut app, Action::Bookmarks(BookmarkAction::SelectNext)).is_empty());
    }

    #[test]
    fn a_repeated_page_does_not_duplicate_rows() {
        let first = bookmark("one", Some("s-a"), None);
        let mut app = with_rows(vec![first.clone()]);
        reduce(
            &mut app,
            Action::Bookmarks(BookmarkAction::Loaded { rows: vec![first], append: true }),
        );
        assert_eq!(app.bookmarks.rows.len(), 1);
    }

    #[test]
    fn enter_opens_the_source_session_at_its_seq() {
        let mut app = with_rows(vec![bookmark("Gateway fix", Some("s-a"), Some(7))]);
        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::OpenSource));
        assert_eq!(app.view(), View::Conversation, "the bookmarks view is left behind");
        assert_eq!(
            app.slice,
            crate::app::slice::Slice::Sessions,
            "and the tab bar follows the conversation"
        );
        assert!(
            effects.iter().any(|e| matches!(e, Effect::LoadConversationPage { .. })),
            "the conversation is opened"
        );
        assert!(app.pins.jump_target() == Some(7), "the jump to the saved seq is armed");
    }

    #[test]
    fn a_bookmark_without_a_seq_just_opens_the_session() {
        let mut app = with_rows(vec![bookmark("No seq", Some("s-a"), None)]);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::OpenSource));
        assert_eq!(app.view(), View::Conversation);
        assert!(app.pins.jump_target().is_none());
    }

    #[test]
    fn a_dead_link_says_the_source_is_gone_and_stays_put() {
        let mut app = with_rows(vec![bookmark("Old plan", None, None)]);
        assert!(reduce(&mut app, Action::Bookmarks(BookmarkAction::OpenSource)).is_empty());
        assert_eq!(app.view(), View::Bookmarks);
        assert_eq!(app.toasts.latest().expect("a toast").text, "source deleted");
    }

    #[test]
    fn a_source_session_that_left_the_list_says_so() {
        let mut app = with_rows(vec![bookmark("Elsewhere", Some("s-gone"), Some(3))]);
        assert!(reduce(&mut app, Action::Bookmarks(BookmarkAction::OpenSource)).is_empty());
        assert_eq!(
            app.toasts.latest().expect("a toast").text,
            "source session is no longer listed"
        );
    }

    #[test]
    fn y_copies_the_shared_markdown_through_the_clipboard() {
        let mut app = with_rows(vec![bookmark("Gateway fix", Some("s-a"), Some(4))]);
        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::CopyMarkdown));
        match effects.as_slice() {
            [Effect::Copy { text, label }] => {
                assert_eq!(*label, "bookmark");
                assert_eq!(
                    text,
                    "# Gateway fix\n\n> note on Gateway fix\n\nthe body of Gateway fix"
                );
            }
            _ => panic!("expected a copy"),
        }
    }

    #[test]
    fn e_edits_the_title_and_the_note_in_one_pass() {
        let mut app = with_rows(vec![bookmark("Gateway fix", Some("s-a"), Some(4))]);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::EditOpen));
        match app.bookmarks.prompt.as_ref() {
            Some(Prompt::Edit { field, title, .. }) => {
                assert_eq!(*field, Field::Title);
                assert_eq!(title, "Gateway fix", "the prompt starts from what is stored");
            }
            _ => panic!("expected an edit prompt"),
        }
        type_text(&mut app, "ed");
        reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptSwitch));
        for _ in 0.."note on Gateway fix".len() {
            reduce(
                &mut app,
                Action::Bookmarks(BookmarkAction::PromptKey(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                ))),
            );
        }
        type_text(&mut app, "keep for release notes");

        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCommit));
        match effects.as_slice() {
            [Effect::UpdateBookmark { title, note, .. }] => {
                assert_eq!(title, "Gateway fixed");
                assert_eq!(note.as_deref(), Some("keep for release notes"));
            }
            _ => panic!("expected an update"),
        }
    }

    #[test]
    fn an_empty_title_is_refused_and_an_empty_note_is_cleared() {
        let mut app = with_rows(vec![bookmark("Gateway fix", Some("s-a"), None)]);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::EditOpen));
        for _ in 0.."Gateway fix".len() {
            reduce(
                &mut app,
                Action::Bookmarks(BookmarkAction::PromptKey(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                ))),
            );
        }
        assert!(reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCommit)).is_empty());
        assert!(app.toasts.latest().is_some());

        reduce(&mut app, Action::Bookmarks(BookmarkAction::EditOpen));
        reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptSwitch));
        for _ in 0.."note on Gateway fix".len() {
            reduce(
                &mut app,
                Action::Bookmarks(BookmarkAction::PromptKey(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                ))),
            );
        }
        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCommit));
        match effects.as_slice() {
            [Effect::UpdateBookmark { note, .. }] => assert!(note.is_none()),
            _ => panic!("expected an update"),
        }
    }

    #[test]
    fn an_update_lands_on_the_row_it_edited() {
        let mut app = with_rows(vec![bookmark("Gateway fix", Some("s-a"), None)]);
        let mut edited = app.bookmarks.rows[0].clone();
        edited.title = "Renamed".to_owned();
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Updated(Box::new(edited))));
        assert_eq!(titles(&app), vec!["Renamed".to_owned()]);
    }

    #[test]
    fn d_deletes_only_behind_a_confirm() {
        let mut app =
            with_rows(vec![bookmark("one", Some("s-a"), None), bookmark("two", Some("s-a"), None)]);
        let doomed = app.bookmarks.rows[0].id;

        reduce(&mut app, Action::Bookmarks(BookmarkAction::DeleteAsk));
        assert_eq!(app.bookmarks.confirm, Some(doomed));
        reduce(&mut app, Action::Bookmarks(BookmarkAction::DeleteCancel));
        assert!(app.bookmarks.confirm.is_none());
        assert!(
            reduce(&mut app, Action::Bookmarks(BookmarkAction::DeleteConfirm)).is_empty(),
            "nothing is deleted without a standing confirm"
        );

        reduce(&mut app, Action::Bookmarks(BookmarkAction::DeleteAsk));
        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::DeleteConfirm));
        match effects.as_slice() {
            [Effect::DeleteBookmark { id }] => assert_eq!(*id, doomed.to_string()),
            _ => panic!("expected a delete"),
        }

        reduce(&mut app, Action::Bookmarks(BookmarkAction::Deleted { id: doomed }));
        assert_eq!(titles(&app), vec!["two".to_owned()]);
        assert_eq!(app.bookmarks.selected, 0);
    }

    #[test]
    fn deleting_the_last_row_keeps_the_selection_in_range() {
        let mut app =
            with_rows(vec![bookmark("one", Some("s-a"), None), bookmark("two", Some("s-a"), None)]);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::SelectNext));
        let doomed = app.bookmarks.rows[1].id;
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Deleted { id: doomed }));
        assert_eq!(app.bookmarks.selected, 0);
        assert_eq!(
            app.bookmarks.selected_bookmark().map(|b| b.title.clone()),
            Some("one".to_owned())
        );
    }

    /// A conversation with the line cursor on the assistant's message.
    fn app_on_a_line() -> App {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        reduce(
            &mut app,
            Action::Conversation(crate::app::conversation::ConversationAction::Loaded {
                session_id: "s-a".to_owned(),
                kind: crate::app::conversation_store::PageKind::Latest,
                claim: None,
                rows: vec![(
                    7,
                    crate::app::state::ConversationLine::new(
                        crate::app::state::LineKind::Assistant,
                        "The gateway fix\n\nmore detail",
                        1_700_000,
                    ),
                )],
                etag: None,
                has_more: false,
            }),
        );
        reduce(
            &mut app,
            Action::Conversation(crate::app::conversation::ConversationAction::ToggleLineCursor),
        );
        app
    }

    fn tab() -> KeyEvent {
        KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)
    }

    #[test]
    fn b_saves_the_focused_message_with_the_payload_the_web_ui_sends() {
        let mut app = app_on_a_line();
        assert_eq!(app.line_cursor, Some(0));
        assert!(reduce(&mut app, Action::Bookmarks(BookmarkAction::Save(tab()))).is_empty());

        match app.bookmarks.prompt.as_ref() {
            Some(Prompt::Save { draft, field }) => {
                assert_eq!(*field, Field::Title);
                assert_eq!(
                    draft.title, "The gateway fix",
                    "the message's first line, as the web titles it"
                );
                assert_eq!(draft.session_id.as_deref(), Some("s-a"));
                assert_eq!(draft.seq, Some(7));
                assert_eq!(draft.role, "assistant");
                assert_eq!(draft.message_ts, 1_700_000_000, "epoch millis on the wire");
                assert_eq!(
                    draft.body, "The gateway fix\n\nmore detail",
                    "the body is the message itself, as the web snapshots it"
                );
                assert!(draft.note.is_none());
            }
            _ => panic!("expected the save form"),
        }

        type_text(&mut app, " summary");
        reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptSwitch));
        type_text(&mut app, "for the notes");
        let effects = reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCommit));
        match effects.as_slice() {
            [Effect::CreateBookmark { draft }] => {
                assert_eq!(draft.title, "The gateway fix summary");
                assert_eq!(draft.note.as_deref(), Some("for the notes"));
            }
            _ => panic!("expected a save"),
        }
        assert!(app.bookmarks.prompt.is_none());
    }

    #[test]
    fn b_outside_line_select_types_into_the_composer_instead() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        assert_eq!(app.line_cursor, None);
        reduce(
            &mut app,
            Action::Bookmarks(BookmarkAction::Save(KeyEvent::new(
                KeyCode::Char('b'),
                KeyModifiers::NONE,
            ))),
        );
        assert!(app.bookmarks.prompt.is_none(), "no save form without a focused line");
        assert!(app.input_active, "the key opened the composer");
        assert_eq!(app.message_input.lines().join("\n"), "b");
    }

    #[test]
    fn an_unsequenced_line_still_saves_as_a_session_back_link() {
        let mut app = app();
        reduce(&mut app, Action::OpenSelectedConversation);
        crate::app::conversation::stream(
            &mut app,
            "s-a",
            None,
            crate::app::state::ConversationLine::new(
                crate::app::state::LineKind::User,
                "typed just now",
                0,
            ),
        );
        reduce(
            &mut app,
            Action::Conversation(crate::app::conversation::ConversationAction::ToggleLineCursor),
        );
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Save(tab())));
        match app.bookmarks.prompt.as_ref() {
            Some(Prompt::Save { draft, .. }) => {
                assert_eq!(draft.seq, None, "no server address yet, but the save still works");
                assert_eq!(draft.role, "user");
            }
            _ => panic!("expected the save form"),
        }
    }

    #[test]
    fn a_saved_bookmark_joins_a_list_that_has_already_been_read() {
        let mut app = with_rows(vec![bookmark("old", Some("s-a"), None)]);
        let fresh = bookmark("brand new", Some("s-a"), Some(2));
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Saved(Box::new(fresh))));
        assert_eq!(titles(&app), vec!["brand new".to_owned(), "old".to_owned()]);
        assert!(app.toasts.latest().expect("a toast").text.contains("brand new"));
    }

    #[test]
    fn a_save_form_with_no_title_is_refused() {
        let mut app = app_on_a_line();
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Save(tab())));
        for _ in 0.."The gateway fix".len() {
            reduce(
                &mut app,
                Action::Bookmarks(BookmarkAction::PromptKey(KeyEvent::new(
                    KeyCode::Backspace,
                    KeyModifiers::NONE,
                ))),
            );
        }
        assert!(reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCommit)).is_empty());
        assert!(app.toasts.latest().is_some());
    }

    #[test]
    fn escaping_the_save_form_saves_nothing() {
        let mut app = app_on_a_line();
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Save(tab())));
        reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptCancel));
        assert!(app.bookmarks.prompt.is_none());
    }

    #[test]
    fn an_age_label_reads_as_the_web_list_does() {
        let now = at(2_000).timestamp_millis() + 2 * 86_400 * 1_000;
        assert_eq!(age_label(at(2_000), now), "2d 0h");
        assert_eq!(age_label(at(2_000), at(2_000).timestamp_millis()), "0m");
    }

    #[test]
    fn the_preview_scroll_stops_at_the_end_of_the_body() {
        let mut app = with_rows(vec![bookmark("one", Some("s-a"), None)]);
        let rows = app.bookmarks.preview_rows();
        assert!(rows > 0);
        for _ in 0..rows + 20 {
            reduce(&mut app, Action::Bookmarks(BookmarkAction::PreviewDown));
        }
        assert_eq!(app.bookmarks.preview_scroll, rows - 1, "it cannot run past the end");

        reduce(&mut app, Action::Bookmarks(BookmarkAction::PreviewUp));
        assert_eq!(app.bookmarks.preview_scroll, rows - 2, "so one key brings it back");

        reduce(&mut app, Action::Bookmarks(BookmarkAction::SelectNext));
        assert_eq!(app.bookmarks.preview_scroll, 0, "moving row resets the preview");
    }

    #[test]
    fn a_failed_load_says_so_and_stops_loading() {
        let mut app = app();
        enter(&mut app);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::Failed));
        assert!(!app.bookmarks.loading);
        assert!(app.toasts.latest().is_some());
        reduce(&mut app, Action::Slice(crate::app::slice::SliceAction::Switch(1)));
        assert!(!enter(&mut app).is_empty(), "a failed first read is retried on the next visit");
    }
}
