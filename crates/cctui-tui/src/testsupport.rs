//! Fixtures here must leave every clock-derived field (`registered_at`,
//! `last_tool_at`, …) unset: anything relative to `now` makes a snapshot drift.

use cctui_proto::api::SessionListItem;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

use crate::app::{App, ConversationLine, LineKind, PendingPermission};

/// Pinned so a version bump cannot rewrite every snapshot.
pub const VERSION: &str = "0.0.0-test";

pub const WIDTH: u16 = 100;
pub const HEIGHT: u16 = 24;

pub fn render_screen(app: &mut App) -> String {
    render_screen_sized(app, WIDTH, HEIGHT)
}

pub fn render_screen_sized(app: &mut App, width: u16, height: u16) -> String {
    app.version = VERSION;
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| crate::views::render(frame, app)).expect("draw");
    buffer_text(terminal.backend().buffer())
}

/// Styles are dropped on purpose: these snapshots guard layout and content, so
/// a palette tweak must not rewrite every one of them.
fn buffer_text(buffer: &ratatui::buffer::Buffer) -> String {
    let area = buffer.area();
    let mut rows: Vec<String> = Vec::with_capacity(area.height as usize);
    for y in area.top()..area.bottom() {
        let mut row = String::new();
        for x in area.left()..area.right() {
            row.push_str(buffer.cell((x, y)).map_or(" ", ratatui::buffer::Cell::symbol));
        }
        rows.push(row.trim_end().to_owned());
    }
    while rows.last().is_some_and(String::is_empty) {
        rows.pop();
    }
    rows.join("\n")
}

pub fn session(id: &str, project: &str, status: &str, bucket: &str) -> SessionListItem {
    serde_json::from_value(json!({
        "id": id,
        "parent_id": null,
        "machine_id": "orion",
        "working_dir": format!("/home/dev/{project}"),
        "status": status,
        "liveness": "active",
        "bucket": bucket,
        "token_usage": {"tokens_in": 12_000, "tokens_out": 3_400, "cost_usd": 1.25},
        "metadata": {"project_name": project, "git_branch": "main", "model": "opus"},
        "adapter_id": "claude-code",
    }))
    .expect("fixture session")
}

pub fn pinned_session(id: &str, project: &str) -> SessionListItem {
    let mut s = session(id, project, "active", "working");
    s.pinned = true;
    s
}

pub fn dispatched_session(id: &str, project: &str, bucket: &str) -> SessionListItem {
    let mut s = session(id, project, "active", bucket);
    s.machine_kind = Some("dispatch".to_owned());
    s
}

pub fn subagent(id: &str, parent_id: &str, project: &str) -> SessionListItem {
    let mut s = session(id, project, "active", "working");
    s.parent_id = Some(parent_id.to_owned());
    s
}

pub fn app_with_sessions() -> App {
    let mut app = App::new();
    app.sessions = vec![
        session("s-working", "cctui", "active", "working"),
        session("s-blocked", "infra", "active", "blocked"),
        subagent("s-child", "s-working", "cctui-sub"),
        session("s-done", "notes", "inactive", "done"),
    ];
    app.update_aggregates();
    app
}

pub fn conversation_lines() -> Vec<ConversationLine> {
    vec![
        line(LineKind::User, "add a snapshot harness"),
        line(LineKind::Assistant, "Looking at the views first.\n\n- one\n- two"),
        ConversationLine {
            timestamp: 0,
            kind: LineKind::ToolCall,
            text: "[Read] crates/cctui-tui/src/main.rs".to_owned(),
            tool_input: None,
        },
        line(LineKind::ToolResult, "  → 984 lines"),
        line(LineKind::System, "⟳ context reset (/clear · /compact)"),
        line(LineKind::Reply, "done"),
    ]
}

pub fn conversation_store() -> crate::app::ConversationStore {
    let mut store = crate::app::ConversationStore::new();
    let mut seq = 0_i64;
    for line in conversation_lines() {
        seq += 1;
        store.push_live(Some(seq), line);
    }
    store
}

fn line(kind: LineKind, text: &str) -> ConversationLine {
    ConversationLine { timestamp: 0, kind, text: text.to_owned(), tool_input: None }
}

pub fn permission_request() -> PendingPermission {
    PendingPermission {
        session_id: "s-working".to_owned(),
        request_id: "req-1".to_owned(),
        tool_name: "Bash".to_owned(),
        description: "Run the workspace test suite".to_owned(),
        input_preview: r#"{"command":"cargo test --workspace"}"#.to_owned(),
    }
}

pub fn edit_permission_request() -> PendingPermission {
    PendingPermission {
        session_id: "s-working".to_owned(),
        request_id: "req-2".to_owned(),
        tool_name: "Edit".to_owned(),
        description: "Edit a file".to_owned(),
        input_preview: serde_json::json!({
            "file_path": "src/main.rs",
            "old_string": "let a = 1;",
            "new_string": "let a = 2;",
        })
        .to_string(),
    }
}
