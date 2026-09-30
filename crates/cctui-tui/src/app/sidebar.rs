//! The right sidebar: the agent's todo list and its subagents.

use cctui_proto::api::{SessionListItem, TodoEntry};
use cctui_proto::models::{Liveness, SessionStatus};

use super::action::Effect;
use super::conversation;
use super::state::{App, View};
use super::toast::Level;

/// Columns the sidebar takes when it is open, and the width below which the
/// terminal is too narrow to give it any.
pub const WIDTH: u16 = 34;
pub const MIN_TERMINAL_WIDTH: u16 = 70;

#[derive(Debug, Default)]
pub struct Sidebar {
    /// Which subagent row the cursor sits on; clamped against the real list.
    pub cursor: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarAction {
    Toggle,
    Close,
    Move(isize),
    OpenChild,
    OpenParent,
}

pub fn reduce_sidebar(app: &mut App, action: SidebarAction) -> Vec<Effect> {
    match action {
        SidebarAction::Toggle => toggle(app),
        SidebarAction::Close => close(app),
        SidebarAction::Move(delta) => {
            move_cursor(app, delta);
            Vec::new()
        }
        SidebarAction::OpenChild => open_child(app),
        SidebarAction::OpenParent => open_parent(app),
    }
}

/// One key: the sidebar appears and takes the keyboard, or goes away. The
/// visibility is remembered between runs; the focus is not.
fn toggle(app: &mut App) -> Vec<Effect> {
    if app.ui.sidebar_open {
        return close(app);
    }
    app.ui.sidebar_open = true;
    app.sidebar.cursor = 0;
    app.router.push(View::Sidebar);
    vec![Effect::SaveUiState(app.ui.clone())]
}

fn close(app: &mut App) -> Vec<Effect> {
    let was_open = app.ui.sidebar_open;
    app.ui.sidebar_open = false;
    if app.view() == View::Sidebar {
        app.router.pop();
    }
    if was_open { vec![Effect::SaveUiState(app.ui.clone())] } else { Vec::new() }
}

fn move_cursor(app: &mut App, delta: isize) {
    let len = children_of(app).len();
    if len == 0 {
        app.sidebar.cursor = 0;
        return;
    }
    let last = len - 1;
    let cursor = app.sidebar.cursor.min(last);
    app.sidebar.cursor = if delta < 0 {
        cursor.checked_sub(1).unwrap_or(last)
    } else if cursor >= last {
        0
    } else {
        cursor + 1
    };
}

fn open_child(app: &mut App) -> Vec<Effect> {
    let Some(id) = children_of(app).get(app.sidebar.cursor).map(|c| c.id.clone()) else {
        return Vec::new();
    };
    conversation::switch_to(app, id)
}

/// `u` works whether or not the sidebar is showing: going up is navigation,
/// not a panel feature.
fn open_parent(app: &mut App) -> Vec<Effect> {
    let Some(parent) = app.selected_session().and_then(|s| s.parent_id.clone()) else {
        app.toast(Level::Info, "this session has no parent");
        return Vec::new();
    };
    if !app.sessions.iter().any(|s| s.id == parent) {
        app.toast(Level::Warn, "the parent session is not in the list");
        return Vec::new();
    }
    conversation::switch_to(app, parent)
}

fn children_of(app: &App) -> Vec<ChildRow> {
    app.selected_session_id().map(|id| child_rows(&app.sessions, &id)).unwrap_or_default()
}

/// One subagent line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildRow {
    pub id: String,
    pub label: String,
    pub state: &'static str,
    /// Still doing something, so it is worth watching.
    pub running: bool,
}

/// Children in list order, which is the order they were spawned in.
#[must_use]
pub fn child_rows(sessions: &[SessionListItem], parent_id: &str) -> Vec<ChildRow> {
    sessions
        .iter()
        .filter(|s| s.parent_id.as_deref() == Some(parent_id))
        .map(|s| ChildRow {
            id: s.id.clone(),
            label: child_label(s),
            state: child_state(s),
            running: is_running(s),
        })
        .collect()
}

fn child_label(s: &SessionListItem) -> String {
    s.name
        .clone()
        .or_else(|| {
            s.metadata.get("project_name").and_then(serde_json::Value::as_str).map(str::to_owned)
        })
        .unwrap_or_else(|| s.id.chars().take(8).collect())
}

/// The coarse state a subagent is in; the classifier's bucket unless something
/// more final has happened to it.
fn child_state(s: &SessionListItem) -> &'static str {
    if s.status == SessionStatus::Archived {
        return "archived";
    }
    if s.end_reason.is_some() {
        return "ended";
    }
    if s.hibernated {
        return "asleep";
    }
    if s.liveness == Liveness::Dead {
        return "dead";
    }
    s.bucket.label()
}

fn is_running(s: &SessionListItem) -> bool {
    s.status != SessionStatus::Archived
        && s.end_reason.is_none()
        && !s.hibernated
        && s.liveness != Liveness::Dead
}

/// Everything the sidebar draws, derived and nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Panel {
    pub todos: Vec<TodoEntry>,
    pub done: usize,
    pub children: Vec<ChildRow>,
    pub running: usize,
    /// The parent's label, when there is a parent in the list to go up to.
    pub parent: Option<String>,
    pub cursor: usize,
}

#[must_use]
pub fn panel(app: &App, session: &SessionListItem) -> Panel {
    let children = child_rows(&app.sessions, &session.id);
    let running = children.iter().filter(|c| c.running).count();
    let cursor = app.sidebar.cursor.min(children.len().saturating_sub(1));
    let parent = session
        .parent_id
        .as_ref()
        .and_then(|id| app.sessions.iter().find(|s| s.id == *id).map(child_label));
    Panel {
        done: session.todos.iter().filter(|t| t.status == "completed").count(),
        todos: session.todos.clone(),
        children,
        running,
        parent,
        cursor,
    }
}

/// The glyph for a todo's state; `in_progress` is the one worth spotting.
#[must_use]
pub fn todo_mark(status: &str) -> &'static str {
    match status {
        "completed" => "[x]",
        "in_progress" => "[~]",
        _ => "[ ]",
    }
}

/// An `in_progress` todo reads better as the gerund the agent wrote.
#[must_use]
pub fn todo_text(todo: &TodoEntry) -> &str {
    if todo.status == "in_progress"
        && let Some(active) = todo.active_form.as_deref().filter(|a| !a.trim().is_empty())
    {
        return active;
    }
    &todo.content
}

#[cfg(test)]
mod tests {
    use super::{SidebarAction, child_rows, panel, todo_mark, todo_text};
    use crate::app::action::Effect;
    use crate::app::{Action, App, View, reduce};
    use crate::testsupport::{session, subagent, todo};

    fn app() -> App {
        let mut app = App::new();
        let mut parent = session("s-parent", "alpha", "active", "working");
        parent.todos = vec![
            todo("completed", "Read the brief", None),
            todo("in_progress", "Wire the panel", Some("Wiring the panel")),
            todo("pending", "Write the tests", None),
        ];
        app.sessions = vec![
            parent,
            subagent("s-kid-1", "s-parent", "reader"),
            subagent("s-kid-2", "s-parent", "writer"),
            session("s-other", "beta", "active", "working"),
        ];
        app.update_aggregates();
        app
    }

    fn sidebar(app: &mut App, action: SidebarAction) -> Vec<Effect> {
        reduce(app, Action::Sidebar(action))
    }

    fn open_parent_conversation(app: &mut App) {
        let index = app
            .flattened_sessions()
            .iter()
            .position(|s| s.id == "s-parent")
            .expect("the parent is listed");
        app.selected_index = index;
        reduce(app, Action::OpenSelectedConversation);
    }

    #[test]
    fn toggling_remembers_the_visibility_and_takes_the_keyboard() {
        let mut app = app();
        open_parent_conversation(&mut app);
        let effects = sidebar(&mut app, SidebarAction::Toggle);
        assert!(app.ui.sidebar_open);
        assert_eq!(app.view(), View::Sidebar);
        assert!(matches!(effects.as_slice(), [Effect::SaveUiState(_)]));

        let effects = sidebar(&mut app, SidebarAction::Toggle);
        assert!(!app.ui.sidebar_open);
        assert_eq!(app.view(), View::Conversation);
        assert!(matches!(effects.as_slice(), [Effect::SaveUiState(_)]));
    }

    #[test]
    fn the_panel_is_the_latest_todo_list_and_the_children() {
        let mut app = app();
        open_parent_conversation(&mut app);
        let session = app.selected_session().cloned().expect("a session");
        let panel = panel(&app, &session);
        assert_eq!(panel.todos.len(), 3);
        assert_eq!(panel.done, 1);
        assert_eq!(panel.children.len(), 2);
        assert_eq!(panel.running, 2);
        assert!(panel.parent.is_none());
        assert_eq!(panel.children[0].label, "reader");
        assert_eq!(panel.children[0].state, "Working");
    }

    #[test]
    fn a_child_reports_the_state_that_has_actually_happened_to_it() {
        let mut app = app();
        app.sessions[1].hibernated = true;
        app.sessions[2].end_reason = Some(cctui_proto::models::SessionEndReason::Completed);
        let rows = child_rows(&app.sessions, "s-parent");
        assert_eq!(rows[0].state, "asleep");
        assert!(!rows[0].running);
        assert_eq!(rows[1].state, "ended");
        assert!(!rows[1].running);
    }

    #[test]
    fn the_cursor_wraps_over_the_children() {
        let mut app = app();
        open_parent_conversation(&mut app);
        sidebar(&mut app, SidebarAction::Move(1));
        assert_eq!(app.sidebar.cursor, 1);
        sidebar(&mut app, SidebarAction::Move(1));
        assert_eq!(app.sidebar.cursor, 0);
        sidebar(&mut app, SidebarAction::Move(-1));
        assert_eq!(app.sidebar.cursor, 1);
    }

    #[test]
    fn enter_opens_the_child_under_the_cursor() {
        let mut app = app();
        open_parent_conversation(&mut app);
        sidebar(&mut app, SidebarAction::Move(1));
        let effects = sidebar(&mut app, SidebarAction::OpenChild);
        assert_eq!(app.selected_session_id().as_deref(), Some("s-kid-2"));
        assert!(effects.iter().any(|e| matches!(e, Effect::Subscribe { .. })));
    }

    #[test]
    fn u_goes_up_to_the_parent_and_says_so_when_there_is_none() {
        let mut app = app();
        let index = app
            .flattened_sessions()
            .iter()
            .position(|s| s.id == "s-kid-1")
            .expect("the child is listed");
        app.selected_index = index;
        reduce(&mut app, Action::OpenSelectedConversation);

        let effects = sidebar(&mut app, SidebarAction::OpenParent);
        assert_eq!(app.selected_session_id().as_deref(), Some("s-parent"));
        assert!(effects.iter().any(|e| matches!(e, Effect::Subscribe { .. })));

        assert!(sidebar(&mut app, SidebarAction::OpenParent).is_empty());
        assert!(app.toasts.latest().expect("a toast").text.contains("no parent"));
    }

    #[test]
    fn a_session_with_no_children_has_nothing_to_open() {
        let mut app = app();
        let index =
            app.flattened_sessions().iter().position(|s| s.id == "s-other").expect("listed");
        app.selected_index = index;
        reduce(&mut app, Action::OpenSelectedConversation);
        sidebar(&mut app, SidebarAction::Move(1));
        assert_eq!(app.sidebar.cursor, 0);
        assert!(sidebar(&mut app, SidebarAction::OpenChild).is_empty());
    }

    #[test]
    fn a_running_todo_reads_as_the_gerund_the_agent_wrote() {
        let app = app();
        let todos = &app.sessions[0].todos;
        assert_eq!(todo_mark(&todos[0].status), "[x]");
        assert_eq!(todo_mark(&todos[1].status), "[~]");
        assert_eq!(todo_mark(&todos[2].status), "[ ]");
        assert_eq!(todo_text(&todos[1]), "Wiring the panel");
        assert_eq!(todo_text(&todos[0]), "Read the brief");
    }
}
