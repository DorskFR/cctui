use crate::app::View;
use crate::testsupport::{
    CLOCK_MS, app_with_sessions, conversation_store, ms_ago, permission_request, render_screen,
    render_screen_sized, session, todo,
};

#[test]
fn session_list() {
    let mut app = app_with_sessions();
    insta::assert_snapshot!(render_screen(&mut app));
}

fn app_with_many_sessions() -> crate::app::App {
    let mut app = app_with_sessions();
    for i in 0..30 {
        app.sessions.push(crate::testsupport::session(
            &format!("s-extra-{i:02}"),
            &format!("extra{i:02}"),
            "active",
            "working",
        ));
    }
    app.update_aggregates();
    app
}

#[test]
fn session_list_scrolls_when_rows_exceed_the_viewport() {
    let mut app = app_with_many_sessions();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_selection_at_the_bottom_scrolls() {
    let mut app = app_with_many_sessions();
    app.select_last();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_pinned_group() {
    let mut app = app_with_sessions();
    app.sessions.push(crate::testsupport::pinned_session("s-pin", "starred"));
    app.update_aggregates();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_dispatched_group() {
    let mut app = app_with_sessions();
    app.sessions.push(crate::testsupport::dispatched_session("s-disp", "worker", "working"));
    app.sessions.push(crate::testsupport::dispatched_session("s-disp-blocked", "wk2", "blocked"));
    app.update_aggregates();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_compact_rows() {
    let mut app = app_with_sessions();
    app.config.prefs.compact_rows = true;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_selection_moves() {
    let mut app = app_with_sessions();
    app.select_next();
    app.select_next();
    insta::assert_snapshot!(render_screen(&mut app));
}

/// One row per derived state: grinding with a task list, wedged, stale,
/// hibernated, ended badly, and one waiting on a permission.
fn app_with_rich_statuses() -> crate::app::App {
    const MIN: i64 = 60 * 1000;
    let mut app = crate::app::App::new();
    app.clock_ms = CLOCK_MS;

    let mut grinding = session("s-grind", "cctui", "active", "working");
    grinding.tool_use_count = 14;
    grinding.last_tool_at = Some(ms_ago(8_000));
    grinding.last_heartbeat = Some(ms_ago(8_000));
    grinding.todos = vec![
        todo("completed", "read the code", None),
        todo("completed", "write the module", None),
        todo("completed", "wire the view", None),
        todo("in_progress", "Run the tests", Some("Running tests")),
        todo("pending", "commit", None),
        todo("pending", "report", None),
        todo("pending", "rest", None),
    ];
    grinding.unread_count = 4;

    let mut asleep = session("s-asleep", "web", "active", "working");
    asleep.tool_use_count = 2;
    asleep.last_tool_at = Some(ms_ago(3 * MIN));
    asleep.last_heartbeat = Some(ms_ago(3 * MIN));

    let mut stale = session("s-stale", "api", "active", "working");
    stale.last_heartbeat = Some(ms_ago(42 * MIN));

    let mut hibernated = session("s-sleep", "notes", "inactive", "done");
    hibernated.hibernated = true;

    let mut ended = session("s-dead", "infra", "inactive", "done");
    ended.end_reason = Some(cctui_proto::models::SessionEndReason::MachineOffline);
    ended.ended_at = Some(ms_ago(5 * MIN));

    let mut waiting = session("s-perm", "deploy", "active", "blocked");
    waiting.auto_approve = true;

    app.sessions = vec![grinding, asleep, stale, hibernated, ended, waiting];
    app.permission_queue.push_back(crate::app::PendingPermission {
        session_id: "s-perm".to_owned(),
        request_id: "req-1".to_owned(),
        tool_name: "Bash".to_owned(),
        description: "Deploy".to_owned(),
        input_preview: "make deploy".to_owned(),
    });
    app.update_aggregates();
    app
}

#[test]
fn session_list_rich_statuses() {
    let mut app = app_with_rich_statuses();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_rich_statuses_at_eighty_columns() {
    let mut app = app_with_rich_statuses();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

/// A 12-agent plain group and a named workflow run under one orchestrator.
fn app_with_subagent_tree() -> crate::app::App {
    let mut app = crate::app::App::new();
    app.sessions = vec![session("s-orch", "cctui", "active", "working")];
    for i in 0..12 {
        app.sessions.push(crate::testsupport::subagent(&format!("s-a{i:02}"), "s-orch", "lane"));
    }
    for (i, lane) in ["lane-a", "lane-b", "lane-c", "lane-d"].iter().enumerate() {
        let mut child = crate::testsupport::subagent(&format!("s-w{i}"), "s-orch", lane);
        child.metadata = serde_json::json!({
            "project_name": lane,
            "workflow_run_id": "run-1",
            "workflow_name": "release-wave",
        });
        app.sessions.push(child);
    }
    app.sessions.push(crate::testsupport::pinned_session("s-pin", "starred"));
    app.update_aggregates();
    app
}

#[test]
fn session_list_subagent_groups_start_folded() {
    let mut app = app_with_subagent_tree();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_subagent_groups_expanded() {
    let mut app = app_with_subagent_tree();
    app.ui.toggle_group("s-orch/plain", 12);
    app.ui.toggle_group("s-orch/wf:run-1", 4);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_sections_folded() {
    let mut app = app_with_subagent_tree();
    app.ui.toggle_section("working");
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_without_data() {
    let mut app = app_with_sessions();
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_narrow() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

#[test]
fn help_overlay() {
    let mut app = app_with_sessions();
    app.router.push(View::Help);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn permission_dialog() {
    let mut app = app_with_sessions();
    app.permission_queue.push_back(permission_request());
    app.router.push(View::PermissionDialog);
    insta::assert_snapshot!(render_screen(&mut app));
}
