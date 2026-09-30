//! Fixtures here must leave every clock-derived field (`registered_at`,
//! `last_tool_at`, …) unset: anything relative to `now` makes a snapshot drift.

use cctui_proto::api::SessionListItem;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use serde_json::json;

use crate::app::{App, ConversationLine, LineKind, PendingPermission, ToolCategory, TurnFooter};

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
    let liveness = if status == "active" { "active" } else { "dead" };
    serde_json::from_value(json!({
        "id": id,
        "parent_id": null,
        "machine_id": "orion",
        "working_dir": format!("/home/dev/{project}"),
        "status": status,
        "liveness": liveness,
        "bucket": bucket,
        "token_usage": {"tokens_in": 12_000, "tokens_out": 3_400, "cost_usd": 1.25},
        "metadata": {"project_name": project, "git_branch": "main", "model": "opus"},
        "adapter_id": "claude-code",
    }))
    .expect("fixture session")
}

/// Pinned clock for rows that carry real timestamps: `now_ms` is a parameter
/// everywhere, so a snapshot fixes it instead of drifting with the wall clock.
pub const CLOCK_MS: i64 = 1_700_000_000_000;

pub fn ms_ago(ms: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp_millis(CLOCK_MS - ms).expect("a valid stamp")
}

pub fn todo(status: &str, content: &str, active_form: Option<&str>) -> cctui_proto::api::TodoEntry {
    cctui_proto::api::TodoEntry {
        content: content.to_owned(),
        status: status.to_owned(),
        active_form: active_form.map(str::to_owned),
    }
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
    let mut peer = line(LineKind::Peer, "the parser lane is rebased");
    peer.peer_from = Some("lane-b".to_owned());

    let mut assistant = line(LineKind::Assistant, "Looking at the views first.\n\n- one\n- two");
    assistant.footer = Some(TurnFooter {
        duration_ms: None,
        tokens_in: Some(12_400),
        tokens_out: Some(1_100),
        needs_action: false,
    });

    let mut summary = line(LineKind::Summary, String::new());
    summary.footer = Some(TurnFooter { duration_ms: Some(38_000), ..TurnFooter::default() });

    vec![
        line(LineKind::User, "add a snapshot harness"),
        line(LineKind::Thinking { redacted: false }, "two parsers\nthe second one wraps"),
        tool(ToolCategory::Read, "Read", "crates/cctui-tui/src/main.rs"),
        result(false, "Read", "984 lines"),
        tool(ToolCategory::Write, "Bash", "cargo test"),
        result(true, "Bash", "exit 101 · 3 failed\n  parser::wraps\n  parser::nests"),
        assistant,
        summary,
        peer,
        line(LineKind::Marker, "keep-alive tick"),
        line(LineKind::Reset, "context reset (/clear)"),
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

fn line(kind: LineKind, text: impl Into<String>) -> ConversationLine {
    ConversationLine::new(kind, text, 0)
}

fn tool(category: ToolCategory, name: &str, detail: &str) -> ConversationLine {
    let mut line = line(LineKind::Tool { category }, detail);
    line.tool = Some(name.to_owned());
    line
}

fn result(error: bool, name: &str, text: &str) -> ConversationLine {
    let mut line = line(LineKind::Result { error }, text);
    line.tool = Some(name.to_owned());
    line
}

pub fn ask_card() -> crate::app::prompt::AskCard {
    let questions = json!([
            {
                "header": "Storage",
                "question": "Which database?",
                "options": [
                    {"label": "Postgres", "description": "the default"},
                    {"label": "SQLite"}
                ]
            },
            {
                "question": "Which features?",
                "multiSelect": true,
                "options": [{"label": "auth"}, {"label": "billing"}]
            }
    ]);
    crate::app::prompt::AskCard::new("Which database?".to_owned(), Some(&questions), None)
}

pub fn plan_card() -> crate::app::prompt::PlanCard {
    crate::app::prompt::PlanCard::new(
        "# Plan\n\n- rework the reducer\n- add the card".to_owned(),
        None,
    )
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

/// `ended_at` is pinned, not `now`: an end badge must not drift a snapshot.
pub fn ended_session(
    id: &str,
    project: &str,
    reason: &str,
    detail: Option<&str>,
) -> SessionListItem {
    let mut s = session(id, project, "inactive", "done");
    s.end_reason = Some(cctui_proto::models::SessionEndReason::parse(reason));
    s.end_detail = detail.map(str::to_owned);
    s.ended_at = chrono::DateTime::from_timestamp_millis(1_700_000_000_000);
    s
}

pub fn diagnose_response() -> cctui_proto::diagnose::SessionDiagnoseResponse {
    use cctui_proto::diagnose::{
        AttachStatus, DiagnoseFact, EffectiveState, ServerDiagnose, SessionDiagnose,
        SessionDiagnoseResponse, SocketStatus,
    };
    let now = CLOCK_MS;
    SessionDiagnoseResponse {
        session_id: "s-working".to_owned(),
        daemon: Some(SessionDiagnose {
            local_id: "s-working".to_owned(),
            short: Some("6e189420".to_owned()),
            generated_at_ms: now,
            adapter: "claude-code".to_owned(),
            effective_state: DiagnoseFact::observed(
                EffectiveState {
                    verdict: "active/working".to_owned(),
                    tempo: Some("active".to_owned()),
                    state: Some("working".to_owned()),
                    detail: Some("running tests".to_owned()),
                    activity: None,
                },
                "activity",
                now - 1_000,
                now,
            ),
            last_hook_event: DiagnoseFact::missing("hook", "no hook delivery seen"),
            attach: DiagnoseFact::undated(
                AttachStatus {
                    phase: "held".to_owned(),
                    backoff_ms: None,
                    last_probe_alive: Some(true),
                    last_probe_at_ms: Some(now - 10_000),
                },
                "attach",
            ),
            pty_output: DiagnoseFact::missing("pty", "PTY capture not implemented"),
            claude_socket: DiagnoseFact::fresh(
                SocketStatus {
                    path: Some("/tmp/cc-daemon-1000/ab/control.sock".to_owned()),
                    live: true,
                    candidates: vec!["/tmp/cc-daemon-1000/ab/control.sock".to_owned()],
                },
                "discovery",
                now,
            ),
            transcript: DiagnoseFact::missing("filesystem", "no transcript pinned"),
            prompts: DiagnoseFact::missing("hook", "no prompt state yet"),
            permission_mode: DiagnoseFact::undated("yolo".to_owned(), "spawn"),
            dispatch: DiagnoseFact::missing("dispatch", "not a dispatched session"),
            gateway: DiagnoseFact::missing("daemon-config", "no server client"),
            codex: None,
            opencode: None,
        }),
        daemon_error: None,
        server: ServerDiagnose {
            status: Some("active".to_owned()),
            adapter_id: Some("claude-code".to_owned()),
            account_bound: true,
            accounts: vec!["main".to_owned()],
            machine_id: Some("orion".to_owned()),
            machine_last_seen_ms: Some(now - 4_000),
        },
        silence: vec![],
    }
}

/// A session whose every dated field is pinned to [`CLOCK_MS`].
pub fn diagnosable_session() -> SessionListItem {
    let mut s = session("s-working", "cctui", "active", "working");
    s.parent_id = None;
    s.machine_name = Some("orion".to_owned());
    s.model = Some("opus".to_owned());
    s.effort = Some("high".to_owned());
    s.permission_mode = Some("yolo".to_owned());
    s.account_name = Some("main".to_owned());
    s.account_traffic_observed = true;
    s.registered_at = Some(ms_ago(3_600_000));
    s.last_heartbeat = Some(ms_ago(2_000));
    s.last_tool_at = Some(ms_ago(45_000));
    s
}
