use cctui_proto::drafts::{Draft, DraftList, session_history_key};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::drafts::DraftAction;
use crate::app::{Action, View, reduce};
use crate::testsupport::{
    CLOCK_MS, app_with_sessions, ask_card, conversation_store, edit_permission_request,
    ended_session, ms_ago, permission_request, plan_card, render_screen, render_screen_sized,
    session, todo,
};

/// `selected_index` walks the grouped list, so the session on screen is not
/// `sessions[0]`: a fixture has to be aimed at the selected one.
fn selected(app: &crate::app::App) -> String {
    app.selected_session_id().expect("a selected session")
}

fn with_permission(app: &mut crate::app::App, mut req: crate::app::PendingPermission) {
    req.session_id = selected(app);
    app.permissions.push(req);
}

fn session_mut<'a>(
    app: &'a mut crate::app::App,
    id: &str,
) -> &'a mut cctui_proto::api::SessionListItem {
    app.sessions.iter_mut().find(|s| s.id == id).expect("a fixture session")
}

/// Call after any mutation that changes a row's group: the flattened order,
/// and with it `selected_index`, moves under the selection.
fn focus(app: &mut crate::app::App, id: &str) {
    let index = app.flattened_sessions().iter().position(|s| s.id == id).expect("a listed session");
    app.selected_index = index;
}

/// A conversation open on one named session, whatever grouping does to the rows.
fn app_on(id: &str) -> crate::app::App {
    let mut app = app_with_sessions();
    app.conversations.insert(id.to_owned(), conversation_store());
    app.router.push(View::Conversation);
    app
}

fn app_in_conversation() -> crate::app::App {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    app
}

#[test]
fn conversation_ask_card() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    app.asks.insert(id, ask_card());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_ask_card_answered_second_question() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    let mut card = ask_card();
    card.chosen[0].insert(0);
    card.current = 1;
    card.chosen[1].insert(1);
    app.asks.insert(id, card);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_ask_card_free_text() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    let mut card = ask_card();
    card.editing_other = true;
    card.other[0] = "mysql".to_owned();
    app.asks.insert(id, card);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_plan_card() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    app.plans.insert(id, plan_card());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_plan_card_refining() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    let mut card = plan_card();
    card.refining = true;
    card.refine = "make it smaller".to_owned();
    app.plans.insert(id, card);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn help_overlay_tall_enough_for_the_glyph_legend() {
    let mut app = app_with_sessions();
    app.router.push(View::Help);
    insta::assert_snapshot!(render_screen_sized(&mut app, 100, 46));
}

#[test]
fn session_list_shows_a_waiting_prompt_marker() {
    let mut app = app_with_sessions();
    app.asks.insert("s-working".to_owned(), ask_card());
    app.plans.insert("s-blocked".to_owned(), plan_card());
    insta::assert_snapshot!(render_screen(&mut app));
}

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
    app.permissions.push(crate::app::PendingPermission {
        session_id: "s-perm".to_owned(),
        request_id: "req-1".to_owned(),
        tool_name: "Bash".to_owned(),
        description: "Deploy".to_owned(),
        input_preview: "make deploy".to_owned(),
    });
    app.asks.insert("s-asleep".to_owned(), ask_card());
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
    let mut app = app_in_conversation();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_expanded_blocks() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    let mut store = conversation_store();
    store.set_all_expanded(true);
    app.conversations.insert(id, store);
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_with_an_unsent_draft() {
    let mut app = app_in_conversation();
    app.input_active = true;
    let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
    reduce(&mut app, Action::InputKey(key));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn history_picker() {
    let mut app = app_in_conversation();
    let id = app.selected_session().expect("a selected session").id.clone();
    let history = Draft {
        key: session_history_key(&id),
        text: "[\"first prompt\", \"second prompt\"]".to_owned(),
        updated_at: chrono::DateTime::from_timestamp(0, 0).expect("epoch"),
    };
    let list = DraftList { drafts: vec![history] };
    reduce(&mut app, Action::Drafts(DraftAction::IndexLoaded(Box::new(list))));
    reduce(&mut app, Action::Drafts(DraftAction::OpenPicker));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_delivery_states() {
    use crate::app::send::{Phase, seed};
    use crate::app::state::{ConversationLine, LineStatus};
    use crate::app::{ConversationStore, LineKind};

    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    let mut store = ConversationStore::new();
    store.push_live(
        Some(1),
        ConversationLine::new(LineKind::User, "deploy the thing", 0)
            .with_status(LineStatus::Queued),
    );
    store.push_live(
        Some(2),
        ConversationLine::new(LineKind::System, "roll back the release", 0)
            .with_status(LineStatus::Removed),
    );
    app.conversations.insert(id.clone(), store);
    seed(&mut app, &id, "tail the logs", Phase::Pending, None);
    seed(&mut app, &id, "restart the worker", Phase::Failed, Some("no daemon connected"));
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_line_cursor() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    app.follow_tail = false;
    app.line_cursor = Some(5);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_with_timestamps() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    app.show_timestamps = true;
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
    let mut app = app_in_conversation();
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

#[test]
fn help_overlay() {
    let mut app = app_with_sessions();
    app.router.push(View::Help);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn help_overlay_scrolled_to_the_end() {
    let mut app = app_with_sessions();
    app.router.push(View::Help);
    app.help_scroll = usize::MAX;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_permission_card() {
    let mut app = app_in_conversation();
    with_permission(&mut app, permission_request());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_permission_card_with_a_diff() {
    let mut app = app_in_conversation();
    with_permission(&mut app, edit_permission_request());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_two_permission_cards_stack() {
    let mut app = app_in_conversation();
    with_permission(&mut app, permission_request());
    with_permission(&mut app, edit_permission_request());
    insta::assert_snapshot!(render_screen(&mut app));
}

/// The card belongs to another session: this one shows only the status-bar
/// indicator, and its composer keeps every keystroke.
#[test]
fn conversation_pending_elsewhere_only_shows_the_indicator() {
    let mut app = app_in_conversation();
    let here = selected(&app);
    let mut req = permission_request();
    req.session_id =
        app.sessions.iter().map(|s| s.id.clone()).find(|id| *id != here).expect("another session");
    app.permissions.push(req);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_permission_card_narrow() {
    let mut app = app_in_conversation();
    with_permission(&mut app, permission_request());
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

#[test]
fn conversation_banner_working() {
    let mut app = app_on("s-working");
    app.clock_ms = 120_000;
    let s = session_mut(&mut app, "s-working");
    s.activity_detail = Some("running the tests".to_owned());
    s.last_activity_at = chrono::DateTime::from_timestamp_millis(105_000);
    focus(&mut app, "s-working");
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_banner_silent() {
    let mut app = app_on("s-working");
    app.clock_ms = 600_000;
    session_mut(&mut app, "s-working").last_activity_at =
        chrono::DateTime::from_timestamp_millis(120_000);
    focus(&mut app, "s-working");
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_banner_waiting() {
    let mut app = app_on("s-working");
    session_mut(&mut app, "s-working").bucket = cctui_proto::classifier::Bucket::Blocked;
    focus(&mut app, "s-working");
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_ended_closes_the_composer() {
    let mut app = app_on("s-working");
    *session_mut(&mut app, "s-working") =
        ended_session("s-working", "cctui", "crashed", Some("exit status 139"));
    focus(&mut app, "s-working");
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_ended_failed_start_carries_its_detail() {
    let mut app = app_on("s-working");
    *session_mut(&mut app, "s-working") =
        ended_session("s-working", "cctui", "spawn_failed", Some("unknown model gpt-nope"));
    focus(&mut app, "s-working");
    insta::assert_snapshot!(render_screen(&mut app));
}

/// Both cards live: the permission request stacks on top and holds the keys,
/// so the ask card below it renders unfocused.
#[test]
fn conversation_permission_card_outranks_an_ask_card() {
    let mut app = app_in_conversation();
    let id = selected(&app);
    app.asks.insert(id, ask_card());
    with_permission(&mut app, permission_request());
    insta::assert_snapshot!(render_screen(&mut app));
}
