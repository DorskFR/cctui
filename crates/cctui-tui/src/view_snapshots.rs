use cctui_proto::drafts::{Draft, DraftList, session_history_key};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::attach::AttachAction;
use crate::app::bookmarks::BookmarkAction;
use crate::app::cmdline::{CmdAction, Mode as CmdMode};
use crate::app::controls::{ControlsAction, PickerColumn};
use crate::app::diagnose::DiagnoseAction;
use crate::app::drafts::DraftAction;
use crate::app::fileview::FileViewAction;
use crate::app::labels::LabelAction;
use crate::app::list_search::ListSearchAction;
use crate::app::list_shape_reduce::ListShapeAction;
use crate::app::macros::MacroAction;
use crate::app::pins::PinAction;
use crate::app::sidebar::SidebarAction;
use crate::app::slice::SliceAction;
use crate::app::unread::UnreadAction;
use crate::app::{Action, View, reduce};
use crate::testsupport::{
    CLOCK_MS, app_with_sessions, ask_card, conversation_store, diagnosable_session,
    diagnose_response, edit_permission_request, ended_session, ms_ago, permission_request,
    picker_models, plan_card, render_screen, render_screen_sized, session, todo,
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

/// Dated fields are pinned to the test clock, so the ages do not drift.
fn bookmark(
    title: &str,
    session: Option<&str>,
    note: Option<&str>,
    age_days: i64,
) -> cctui_proto::api::bookmarks::Bookmark {
    cctui_proto::api::bookmarks::Bookmark {
        id: uuid::Uuid::from_u128(u128::try_from(age_days).unwrap_or(0) + 1),
        session_id: session.map(str::to_owned),
        seq: Some(4),
        message_id: None,
        title: title.to_owned(),
        body: format!(
            "## {title}\n\nThe saved body, with a `code` span and a list:\n\n- one\n- two"
        ),
        role: "assistant".to_owned(),
        session_name: session.map(str::to_owned),
        note: note.map(str::to_owned),
        message_ts: crate::testsupport::ms_ago(age_days * 86_400_000),
        created_at: crate::testsupport::ms_ago(age_days * 86_400_000),
    }
}

fn app_with_bookmarks() -> crate::app::App {
    let mut app = app_with_sessions();
    app.clock_ms = crate::testsupport::CLOCK_MS;
    reduce(&mut app, Action::Slice(SliceAction::Switch(2)));
    reduce(
        &mut app,
        Action::Bookmarks(BookmarkAction::Loaded {
            rows: vec![
                bookmark(
                    "Gateway fix summary",
                    Some("fix-auth"),
                    Some("keep for release notes"),
                    2,
                ),
                bookmark("Old plan", None, None, 9),
                bookmark("Auth rollout checklist", Some("cctui"), Some("auth, step by step"), 14),
            ],
            append: false,
        }),
    );
    app
}

#[test]
fn bookmarks_list() {
    let mut app = app_with_bookmarks();
    insta::assert_snapshot!(render_screen(&mut app));
}

/// The wave's row budget: a bookmark row has to stay readable at 80 columns.
#[test]
fn bookmarks_list_at_eighty_columns() {
    let mut app = app_with_bookmarks();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn bookmarks_search_highlights_the_terms() {
    let mut app = app_with_bookmarks();
    reduce(&mut app, Action::Bookmarks(BookmarkAction::SearchOpen));
    for c in "auth".chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptKey(key)));
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn bookmarks_delete_confirm() {
    let mut app = app_with_bookmarks();
    reduce(&mut app, Action::Bookmarks(BookmarkAction::DeleteAsk));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn bookmarks_edit_prompt() {
    let mut app = app_with_bookmarks();
    reduce(&mut app, Action::Bookmarks(BookmarkAction::EditOpen));
    reduce(&mut app, Action::Bookmarks(BookmarkAction::PromptSwitch));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn bookmarks_empty() {
    let mut app = app_with_sessions();
    reduce(&mut app, Action::Slice(SliceAction::Switch(2)));
    reduce(&mut app, Action::Bookmarks(BookmarkAction::Loaded { rows: Vec::new(), append: false }));
    insta::assert_snapshot!(render_screen(&mut app));
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
    // The whole sheet in two columns, plus the overlay's margin and border.
    let rows = crate::views::help::rows_per_column(&app.config.keys);
    let height = u16::try_from(rows + 4).expect("a sheet that fits a terminal");
    insta::assert_snapshot!(render_screen_sized(&mut app, 100, height));
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

#[test]
fn session_list_sections_popup() {
    let mut app = app_with_sessions();
    reduce(&mut app, Action::ListShape(ListShapeAction::ToggleSectionsMenu));
    reduce(&mut app, Action::ListShape(ListShapeAction::SectionsNext));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_grouped_by_machine_with_accents() {
    let mut app = app_with_sessions();
    app.sessions[1].machine_id = "cyberia".to_owned();
    app.sessions[1].machine_name = Some("cyberia".to_owned());
    app.list_shape.group_by = crate::app::list_view::GroupBy::Machine;
    app.list_shape.color_by = crate::app::list_view::ColorBy::Machine;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_sorted_by_name_ascending() {
    let mut app = app_with_sessions();
    reduce(&mut app, Action::ListShape(ListShapeAction::CycleSort));
    reduce(&mut app, Action::ListShape(ListShapeAction::CycleSort));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_with_archived_shown() {
    let mut app = app_with_sessions();
    let mut old = crate::testsupport::session("s-arch", "retired", "archived", "done");
    old.status = cctui_proto::models::SessionStatus::Archived;
    app.sessions.push(old);
    app.list_shape.sections.toggle(crate::app::list_view::Section::Archived);
    app.update_aggregates();
    insta::assert_snapshot!(render_screen(&mut app));
}

fn searching(query: &str) -> crate::app::App {
    let mut app = app_with_sessions();
    reduce(&mut app, Action::ListSearch(ListSearchAction::Open));
    for c in query.chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        reduce(&mut app, Action::ListSearch(ListSearchAction::Key(key)));
    }
    app
}

fn search_hit(
    id: &str,
    project: &str,
    snippet: &str,
    seq: i64,
) -> cctui_proto::api::SessionListItem {
    let mut s = crate::testsupport::session(id, project, "active", "working");
    s.match_snippet = Some(snippet.to_owned());
    s.match_seq = Some(seq);
    s
}

#[test]
fn session_list_search_prompt_before_any_reply() {
    let mut app = searching("machine:cyberia auth");
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_search_results_with_highlighted_snippets() {
    let mut app = searching("auth token");
    reduce(
        &mut app,
        Action::ListSearch(ListSearchAction::Loaded {
            query: "auth token".to_owned(),
            offset: 0,
            sessions: vec![
                search_hit("s-1", "gateway", "refresh the auth token before the gateway", 42),
                search_hit("s-2", "api", "the auth token expired", 7),
            ],
            has_more: false,
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_search_with_no_match() {
    let mut app = searching("nothing matches this");
    reduce(
        &mut app,
        Action::ListSearch(ListSearchAction::Loaded {
            query: "nothing matches this".to_owned(),
            offset: 0,
            sessions: vec![],
            has_more: false,
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_search_including_archived() {
    let mut app = searching("auth");
    reduce(&mut app, Action::ListSearch(ListSearchAction::ToggleArchived));
    reduce(
        &mut app,
        Action::ListSearch(ListSearchAction::Loaded {
            query: "auth".to_owned(),
            offset: 0,
            sessions: vec![search_hit("s-1", "gateway", "the auth flow", 1)],
            has_more: true,
        }),
    );
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

/// `U`: only the rows with something new, and the header says the filter is on.
#[test]
fn session_list_unread_only() {
    let mut app = app_with_rich_statuses();
    reduce(&mut app, Action::Unread(UnreadAction::ToggleOnly));
    insta::assert_snapshot!(render_screen(&mut app));
}

/// Select mode: the checkbox gutter, the picked rows and the strip that
/// replaces the hotkey hints.
/// The spawn dialog with this lane's sections filled in, driven through the
/// keyboard: the account picker open and filtered to the harness, a label on,
/// an env secret masked, a file staged.
#[test]
fn spawn_dialog_account_labels_env_files() {
    use crate::app::spawn::{SpawnAction, SpawnFetch};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let tmp = tempfile::tempdir().expect("tempdir");
    let file = tmp.path().join("trace.log");
    std::fs::write(&file, vec![b'x'; 12 * 1024]).expect("write");

    let mut app = app_with_sessions();
    app.labels.all = vec![
        cctui_proto::api::Label { id: "l1".into(), name: "cct".into(), color: String::new() },
        cctui_proto::api::Label { id: "l2".into(), name: "infra".into(), color: String::new() },
    ];
    crate::app::reduce(&mut app, crate::app::Action::Spawn(SpawnAction::Open));

    for fetch in [
        SpawnFetch::Accounts(
            serde_json::from_value(serde_json::json!([
                {"id": "a1", "name": "personal-max", "providers": [{"provider": "anthropic"}]},
                {"id": "a2", "name": "work-team", "providers": [{"provider": "openai"}]},
            ]))
            .expect("accounts"),
        ),
        SpawnFetch::Pools(
            serde_json::from_value(serde_json::json!([
                {"id": "p1", "name": "personal", "members": [{"account_id": "a1"}]},
            ]))
            .expect("pools"),
        ),
        SpawnFetch::Usage(
            serde_json::from_value(serde_json::json!([
                {"account": "a1", "windows": [{"key": "session", "utilization": 62.0}]},
            ]))
            .expect("usage"),
        ),
    ] {
        crate::app::reduce(
            &mut app,
            crate::app::Action::Spawn(SpawnAction::DataLoaded(Box::new(fetch))),
        );
    }

    let press = |app: &mut crate::app::App, code: KeyCode| {
        crate::app::reduce(
            app,
            crate::app::Action::Spawn(SpawnAction::Key(KeyEvent::new(code, KeyModifiers::NONE))),
        );
    };
    let tab = |app: &mut crate::app::App| {
        crate::app::reduce(app, crate::app::Action::Spawn(SpawnAction::NextField));
    };
    let type_text = |app: &mut crate::app::App, text: &str| {
        for c in text.chars() {
            crate::app::reduce(
                app,
                crate::app::Action::Spawn(SpawnAction::Key(KeyEvent::new(
                    KeyCode::Char(c),
                    KeyModifiers::NONE,
                ))),
            );
        }
    };

    // Tab off the core rows onto the account picker, then pick the anthropic
    // account; the openai one is filtered out under claude-code.
    let core_rows = {
        let form = app.spawn.as_ref().expect("the dialog is open");
        form.sections[0].rows(&form.fields)
    };
    for _ in 0..core_rows {
        tab(&mut app);
    }
    press(&mut app, KeyCode::Char(' '));
    press(&mut app, KeyCode::Char('k'));
    press(&mut app, KeyCode::Enter);
    press(&mut app, KeyCode::Char(' '));

    tab(&mut app);
    press(&mut app, KeyCode::Char(' '));

    tab(&mut app);
    press(&mut app, KeyCode::Char('a'));
    type_text(&mut app, "gh_token");
    press(&mut app, KeyCode::Tab);
    type_text(&mut app, "ghp_secret");

    tab(&mut app);
    press(&mut app, KeyCode::Char('o'));
    type_text(&mut app, file.to_str().expect("utf8"));
    press(&mut app, KeyCode::Enter);

    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_select_mode() {
    use crate::app::row_actions::RowAction;

    let mut app = app_with_sessions();
    crate::app::reduce(&mut app, crate::app::Action::RowAction(RowAction::ToggleSelect));
    crate::app::reduce(&mut app, crate::app::Action::SelectNext);
    crate::app::reduce(&mut app, crate::app::Action::RowAction(RowAction::ToggleSelect));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_batch_archive_confirm() {
    use crate::app::row_actions::RowAction;

    let mut app = app_with_sessions();
    crate::app::reduce(&mut app, crate::app::Action::RowAction(RowAction::SelectAllVisible));
    crate::app::reduce(&mut app, crate::app::Action::RowAction(RowAction::ArchiveOrUnarchive));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_rename_field() {
    use crate::app::row_actions::RowAction;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = app_with_sessions();
    crate::app::reduce(&mut app, crate::app::Action::RowAction(RowAction::RenameStart));
    for c in "fix-auth".chars() {
        crate::app::reduce(
            &mut app,
            crate::app::Action::RowAction(RowAction::RenameKey(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            ))),
        );
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

/// The checkbox gutter comes out of the same 80-column budget the rich row
/// already fights over.
#[test]
fn session_list_select_mode_at_eighty_columns() {
    use crate::app::row_actions::RowAction;

    let mut app = app_with_rich_statuses();
    crate::app::reduce(&mut app, crate::app::Action::RowAction(RowAction::SelectAllVisible));
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
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

/// A pane whose feed came in as one relayed chunk, with the frame and the
/// cursor line a real agent TUI paints.
#[test]
fn terminal_pane() {
    use base64::Engine as _;

    let mut app = app_in_conversation();
    crate::app::reduce(
        &mut app,
        crate::app::Action::Terminal(crate::app::terminal::TerminalAction::Toggle),
    );
    let screen = concat!(
        "\u{1b}[2J\u{1b}[H",
        "╭─ claude ─────────────╮\r\n",
        "│ > run the tests      │\r\n",
        "╰──────────────────────╯\r\n",
        "\u{1b}[1mRunning 42 tests\u{1b}[0m\r\n",
        "  ✓ every one of them",
    );
    let pane = app.terminal.as_mut().expect("an open pane");
    assert!(pane.feed(&base64::engine::general_purpose::STANDARD.encode(screen)));
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
fn pins_list() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    reduce(&mut app, Action::Pins(PinAction::Loaded { session_id: id, seqs: vec![1, 3, 99] }));
    reduce(&mut app, Action::Pins(PinAction::OpenList));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_with_a_pinned_line() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    reduce(&mut app, Action::Pins(PinAction::Loaded { session_id: id, seqs: vec![1, 2] }));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn macros_picker() {
    let mut app = app_in_conversation();
    app.macros = crate::app::macros::from_settings(&serde_json::json!({
        "macros": {
            "enabled": true,
            "items": [
                { "id": "m1", "title": "Triage", "prompt": "triage the inbox and file what matters" },
                { "id": "m2", "title": "Release", "prompt": "cut a release", "adapter": "codex" },
            ]
        }
    }));
    reduce(&mut app, Action::Macros(MacroAction::Open));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn composer_session_mention_popup() {
    let mut app = app_in_conversation();
    app.input_active = true;
    for c in "ping #".chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        reduce(&mut app, Action::InputKey(key));
    }
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

fn search_for(app: &mut crate::app::App, query: &str) {
    use crate::app::cmdline::{CmdAction, Mode};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    reduce(app, Action::CmdLine(CmdAction::Open(Mode::Search)));
    for c in query.chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        reduce(app, Action::CmdLine(CmdAction::Key(key)));
    }
}

#[test]
fn conversation_search_prompt_highlights_as_it_is_typed() {
    let mut app = app_in_conversation();
    search_for(&mut app, "parser");
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_search_committed_shows_the_hit_count() {
    use crate::app::cmdline::CmdAction;

    let mut app = app_in_conversation();
    search_for(&mut app, "parser");
    reduce(&mut app, Action::CmdLine(CmdAction::Commit));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_command_prompt() {
    use crate::app::cmdline::{CmdAction, Mode};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    let mut app = app_in_conversation();
    reduce(&mut app, Action::CmdLine(CmdAction::Open(Mode::Command)));
    for c in "export md".chars() {
        let key = KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE);
        reduce(&mut app, Action::CmdLine(CmdAction::Key(key)));
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_filter_menu() {
    use crate::app::cmdline::CmdAction;

    let mut app = app_in_conversation();
    reduce(&mut app, Action::CmdLine(CmdAction::ToggleFilterMenu));
    reduce(&mut app, Action::CmdLine(CmdAction::FilterMenuNext));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_quick_filter_narrows_to_the_assistant() {
    use crate::app::cmdline::CmdAction;

    let mut app = app_in_conversation();
    reduce(&mut app, Action::CmdLine(CmdAction::CycleFilter));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_show_all_reveals_the_hidden_categories() {
    use crate::app::cmdline::CmdAction;

    let mut app = app_in_conversation();
    reduce(&mut app, Action::CmdLine(CmdAction::FilterShowAll));
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

/// The panel over the session list, with the report already in hand.
fn app_with_panel(mode: crate::app::diagnose::DiagnoseMode) -> crate::app::App {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    *session_mut(&mut app, "s-working") = diagnosable_session();
    focus(&mut app, "s-working");
    reduce(&mut app, Action::Diagnose(DiagnoseAction::Open(mode)));
    reduce(
        &mut app,
        Action::Diagnose(DiagnoseAction::Loaded {
            session_id: "s-working".to_owned(),
            report: Box::new(diagnose_response()),
        }),
    );
    app
}

#[test]
fn diagnose_panel() {
    let mut app = app_with_panel(crate::app::diagnose::DiagnoseMode::Facts);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn diagnose_panel_scrolled() {
    let mut app = app_with_panel(crate::app::diagnose::DiagnoseMode::Facts);
    reduce(&mut app, Action::Diagnose(DiagnoseAction::Scroll(6)));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn diagnose_panel_still_fetching() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    focus(&mut app, "s-working");
    reduce(
        &mut app,
        Action::Diagnose(DiagnoseAction::Open(crate::app::diagnose::DiagnoseMode::Facts)),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn diagnose_panel_with_an_unreachable_daemon() {
    let mut app = app_with_panel(crate::app::diagnose::DiagnoseMode::Facts);
    let mut report = diagnose_response();
    report.daemon = None;
    report.daemon_error = Some("machine orion is offline".to_owned());
    report.server.account_bound = false;
    reduce(
        &mut app,
        Action::Diagnose(DiagnoseAction::Loaded {
            session_id: "s-working".to_owned(),
            report: Box::new(report),
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn diagnose_panel_lists_the_silence_reasons() {
    let mut app = app_with_panel(crate::app::diagnose::DiagnoseMode::Facts);
    let mut report = diagnose_response();
    if let Some(daemon) = report.daemon.as_mut() {
        daemon.adapter = "codex".to_owned();
        daemon.codex = Some(codex_section());
    }
    report.silence = vec![
        cctui_proto::silence::SilenceReason::CodexStalledRpc { count: 2, age_ms: 120_000 },
        cctui_proto::silence::SilenceReason::CodexNoTurn,
    ];
    reduce(
        &mut app,
        Action::Diagnose(DiagnoseAction::Loaded {
            session_id: "s-working".to_owned(),
            report: Box::new(report),
        }),
    );
    reduce(&mut app, Action::Diagnose(DiagnoseAction::Scroll(9)));
    insta::assert_snapshot!(render_screen(&mut app));
}

fn codex_section() -> cctui_proto::diagnose::CodexDiagnose {
    cctui_proto::diagnose::CodexDiagnose {
        codex_version: Some("0.153.4".to_owned()),
        min_version: "0.153.4".to_owned(),
        version_supported: Some(true),
        transport: "stdio".to_owned(),
        app_server_pid: Some(4242),
        live: true,
        registered: true,
        thread_id: Some("019e6628".to_owned()),
        active_turn_id: Some("turn-1".to_owned()),
        turn_status: "working".to_owned(),
        pending_rpc_count: 2,
        pending_rpc_methods: vec!["turn/start".to_owned()],
        protocol_errors: vec![],
        stderr_tail: vec![],
        rpc_tail: vec![],
        rollout_path: None,
        rollout_size_bytes: None,
        auth_state: Some("gateway env present".to_owned()),
        registry_live_mismatch: None,
    }
}

#[test]
fn session_info_popup() {
    let mut app = app_with_panel(crate::app::diagnose::DiagnoseMode::Info);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_info_popup_for_an_ended_session() {
    let mut app = app_with_panel(crate::app::diagnose::DiagnoseMode::Info);
    let row = session_mut(&mut app, "s-working");
    row.end_reason = Some(cctui_proto::models::SessionEndReason::DaemonLost);
    row.end_detail = Some("machine went away".to_owned());
    row.account_traffic_observed = false;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_info_popup_narrow() {
    let mut app = app_with_panel(crate::app::diagnose::DiagnoseMode::Info);
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

#[test]
fn session_list_secondary_badges() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    session_mut(&mut app, "s-working").cache_cold = true;
    let blocked = session_mut(&mut app, "s-blocked");
    blocked.account_name = Some("main".to_owned());
    blocked.account_traffic_observed = false;
    app.soft_limited.insert("s-done".to_owned());
    insta::assert_snapshot!(render_screen(&mut app));
}

// --- Attachments and the linked-file viewer ---

/// A session with a pasted text file and an image staged, as `Ctrl-O` and a
/// large paste leave it.
fn app_with_attachments() -> crate::app::App {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    let _ = reduce(
        &mut app,
        Action::Attach(AttachAction::Read {
            session_id: id.clone(),
            name: "paste-1.txt".to_owned(),
            bytes: vec![b'x'; 12 * 1024],
            content_type: "text/plain".to_owned(),
            dimensions: None,
        }),
    );
    let _ = reduce(
        &mut app,
        Action::Attach(AttachAction::Read {
            session_id: id,
            name: "shot.png".to_owned(),
            bytes: vec![0; 340 * 1024],
            content_type: "image/png".to_owned(),
            dimensions: Some((1280, 720)),
        }),
    );
    app
}

#[test]
fn conversation_attachment_chips() {
    let mut app = app_with_attachments();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_attachment_chip_focused() {
    let mut app = app_with_attachments();
    let _ = reduce(&mut app, Action::Attach(AttachAction::FocusChips));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_attachment_chips_ascii() {
    let mut app = app_with_attachments();
    app.config.prefs.ascii_glyphs = true;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_attachment_cap_error() {
    let mut app = app_in_conversation();
    let id = app.selected_session_id().expect("a selected session");
    let _ = reduce(
        &mut app,
        Action::Attach(AttachAction::Read {
            session_id: id,
            name: "huge.bin".to_owned(),
            bytes: vec![0; 6 * 1024 * 1024],
            content_type: "application/octet-stream".to_owned(),
            dimensions: None,
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn attach_command_line() {
    let mut app = app_in_conversation();
    reduce(&mut app, Action::CmdLine(CmdAction::OpenPrefilled(CmdMode::Command, "attach ")));
    app.cmdline.input = "attach /home/dev/cctui/README".to_owned();
    insta::assert_snapshot!(render_screen(&mut app));
}

fn app_viewing(name: &str, content_type: &str, body: &[u8]) -> crate::app::App {
    let mut app = app_in_conversation();
    let _ = reduce(
        &mut app,
        Action::FileView(FileViewAction::Opened {
            name: name.to_owned(),
            path: format!("/home/dev/{name}"),
            content_type: content_type.to_owned(),
            bytes: body.to_vec(),
        }),
    );
    app
}

#[test]
fn file_viewer_highlights_source() {
    let source = "fn main() {\n    let x = 1;\n    println!(\"{x}\");\n}\n";
    let mut app = app_viewing("main.rs", "text/plain", source.as_bytes());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn file_viewer_renders_markdown() {
    let doc = "# Title\n\nSome **bold** text and a list:\n\n- one\n- two\n";
    let mut app = app_viewing("NOTES.md", "text/markdown", doc.as_bytes());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn file_viewer_scrolled() {
    use std::fmt::Write as _;
    let mut source = String::new();
    for i in 1..=40 {
        let _ = writeln!(source, "let line_{i} = {i};");
    }
    let mut app = app_viewing("long.rs", "text/plain", source.as_bytes());
    let _ = reduce(&mut app, Action::FileView(FileViewAction::Scroll(20)));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn file_viewer_image_placeholder() {
    let mut app = app_viewing("shot.png", "image/png", &[0; 4096]);
    insta::assert_snapshot!(render_screen(&mut app));
}

fn codex_conversation() -> crate::app::App {
    let mut app = app_on("s-working");
    let s = session_mut(&mut app, "s-working");
    s.adapter_id = Some(cctui_proto::adapter::AdapterId::new("codex"));
    s.model = Some("gpt-5.6-sol".to_owned());
    s.effort = Some("high".to_owned());
    s.permission_mode = Some("acceptEdits".to_owned());
    focus(&mut app, "s-working");
    app
}

#[test]
fn conversation_header_carries_the_dials_and_permission_mode() {
    let mut app = codex_conversation();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_interrupt_armed_by_one_press() {
    let mut app = app_on("s-working");
    focus(&mut app, "s-working");
    reduce(&mut app, Action::Controls(ControlsAction::Interrupt));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_interrupt_in_flight() {
    let mut app = app_on("s-working");
    focus(&mut app, "s-working");
    reduce(&mut app, Action::Controls(ControlsAction::Interrupt));
    reduce(&mut app, Action::Controls(ControlsAction::Interrupt));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_fork_armed_by_one_press() {
    let mut app = app_on("s-working");
    focus(&mut app, "s-working");
    reduce(&mut app, Action::Controls(ControlsAction::Fork));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn model_picker_while_the_lists_load() {
    let mut app = codex_conversation();
    reduce(&mut app, Action::Controls(ControlsAction::OpenModelPicker));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn model_picker_loaded() {
    let mut app = codex_conversation();
    reduce(&mut app, Action::Controls(ControlsAction::OpenModelPicker));
    reduce(&mut app, Action::Controls(ControlsAction::ModelsLoaded(Box::new(picker_models()))));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn model_picker_on_the_effort_column() {
    let mut app = codex_conversation();
    reduce(&mut app, Action::Controls(ControlsAction::OpenModelPicker));
    reduce(&mut app, Action::Controls(ControlsAction::ModelsLoaded(Box::new(picker_models()))));
    reduce(&mut app, Action::Controls(ControlsAction::PickerColumn(PickerColumn::Effort)));
    reduce(&mut app, Action::Controls(ControlsAction::PickerMove(1)));
    insta::assert_snapshot!(render_screen(&mut app));
}

/// A gated model shows why it cannot be picked and the footer says so.
#[test]
fn model_picker_on_a_gated_model() {
    let mut app = codex_conversation();
    reduce(&mut app, Action::Controls(ControlsAction::OpenModelPicker));
    reduce(&mut app, Action::Controls(ControlsAction::ModelsLoaded(Box::new(picker_models()))));
    reduce(&mut app, Action::Controls(ControlsAction::PickerMove(1)));
    insta::assert_snapshot!(render_screen(&mut app));
}

fn app_with_a_subagent() -> crate::app::App {
    let mut app = app_on("s-working");
    let s = session_mut(&mut app, "s-working");
    s.todos = vec![
        todo("completed", "Read the brief", None),
        todo("in_progress", "Wire the panel", Some("Wiring the panel")),
        todo("pending", "Write the tests", None),
    ];
    focus(&mut app, "s-working");
    app
}

#[test]
fn conversation_sidebar_open() {
    let mut app = app_with_a_subagent();
    reduce(&mut app, Action::Sidebar(SidebarAction::Toggle));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_sidebar_cursor_on_the_second_subagent() {
    let mut app = app_with_a_subagent();
    session_mut(&mut app, "s-child").hibernated = true;
    let mut extra = crate::testsupport::subagent("s-child-2", "s-working", "writer");
    extra.bucket = cctui_proto::classifier::Bucket::Blocked;
    app.sessions.push(extra);
    focus(&mut app, "s-working");
    reduce(&mut app, Action::Sidebar(SidebarAction::Toggle));
    reduce(&mut app, Action::Sidebar(SidebarAction::Move(1)));
    insta::assert_snapshot!(render_screen(&mut app));
}

/// From a child the panel offers the way back up.
#[test]
fn conversation_sidebar_on_a_subagent() {
    let mut app = app_on("s-child");
    focus(&mut app, "s-child");
    reduce(&mut app, Action::Sidebar(SidebarAction::Toggle));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_sidebar_with_nothing_to_show() {
    let mut app = app_on("s-blocked");
    focus(&mut app, "s-blocked");
    reduce(&mut app, Action::Sidebar(SidebarAction::Toggle));
    insta::assert_snapshot!(render_screen(&mut app));
}

/// Too narrow for a column: the transcript keeps the width.
#[test]
fn conversation_sidebar_suppressed_when_narrow() {
    let mut app = app_with_a_subagent();
    reduce(&mut app, Action::Sidebar(SidebarAction::Toggle));
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

// --- Labels and machines ---

fn label(id: &str, name: &str, color: &str) -> cctui_proto::api::Label {
    cctui_proto::api::Label { id: id.to_owned(), name: name.to_owned(), color: color.to_owned() }
}

/// Rows carrying one, two and four labels, so the `+N` cut is visible.
fn app_with_labels() -> crate::app::App {
    let mut app = app_with_sessions();
    let catalogue = vec![
        label("l-5", "wave-5", "210"),
        label("l-i", "infra", ""),
        label("l-u", "urgent", "0"),
        label("l-d", "docs", "120"),
    ];
    let _ = reduce(&mut app, Action::Labels(LabelAction::Loaded(catalogue.clone())));
    session_mut(&mut app, "s-working").labels = vec![catalogue[0].clone()];
    session_mut(&mut app, "s-blocked").labels = vec![catalogue[1].clone(), catalogue[2].clone()];
    session_mut(&mut app, "s-done").labels = catalogue;
    app
}

#[test]
fn session_list_label_chips() {
    let mut app = app_with_labels();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_label_chips_at_eighty_columns() {
    let mut app = app_with_labels();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn label_picker() {
    let mut app = app_with_labels();
    let _ = reduce(&mut app, Action::Labels(LabelAction::OpenPicker));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn label_picker_filtered() {
    let mut app = app_with_labels();
    let _ = reduce(&mut app, Action::Labels(LabelAction::OpenPicker));
    for c in "wa".chars() {
        let _ = reduce(&mut app, Action::Labels(LabelAction::FilterKey(c)));
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn label_picker_creating() {
    let mut app = app_with_labels();
    let _ = reduce(&mut app, Action::Labels(LabelAction::OpenPicker));
    let _ = reduce(&mut app, Action::Labels(LabelAction::StartCreate));
    for c in "wave-7".chars() {
        let _ = reduce(&mut app, Action::Labels(LabelAction::NameKey(c)));
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn label_picker_hue_choice() {
    let mut app = app_with_labels();
    let _ = reduce(&mut app, Action::Labels(LabelAction::OpenPicker));
    let _ = reduce(&mut app, Action::Labels(LabelAction::StartCreate));
    for c in "wave-7".chars() {
        let _ = reduce(&mut app, Action::Labels(LabelAction::NameKey(c)));
    }
    let _ = reduce(&mut app, Action::Labels(LabelAction::Commit));
    let _ = reduce(&mut app, Action::Labels(LabelAction::HueNext));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn label_picker_confirming_a_delete() {
    let mut app = app_with_labels();
    let _ = reduce(&mut app, Action::Labels(LabelAction::OpenPicker));
    let _ = reduce(&mut app, Action::Labels(LabelAction::StartDelete));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn label_filter_overlay() {
    let mut app = app_with_labels();
    let _ = reduce(&mut app, Action::Labels(LabelAction::OpenFilter));
    let _ = reduce(&mut app, Action::Labels(LabelAction::FilterToggle));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_narrowed_by_a_label_filter() {
    let mut app = app_with_labels();
    let _ = reduce(&mut app, Action::Labels(LabelAction::OpenFilter));
    let _ = reduce(&mut app, Action::Labels(LabelAction::FilterToggle));
    let _ = reduce(&mut app, Action::Labels(LabelAction::CloseFilter));
    insta::assert_snapshot!(render_screen(&mut app));
}

/// Three machines, one of each liveness tier, so the header dot is reviewable.
fn app_with_machines() -> crate::app::App {
    use cctui_proto::models::MachineLiveness;
    let mut app = app_with_sessions();
    for (id, name, hue) in [
        ("s-working", "cyberia-1", 210_i16),
        ("s-blocked", "cyberia-2", 30),
        ("s-done", "orion", 120),
    ] {
        let s = session_mut(&mut app, id);
        s.machine_id = name.to_owned();
        s.machine_name = Some(name.to_owned());
        s.machine_hue = Some(hue);
    }
    session_mut(&mut app, "s-child").machine_id = "cyberia-1".to_owned();
    session_mut(&mut app, "s-child").machine_name = Some("cyberia-1".to_owned());
    app.machine_liveness.insert("cyberia-1".to_owned(), MachineLiveness::Online);
    app.machine_liveness.insert("cyberia-2".to_owned(), MachineLiveness::Stale);
    app.machine_liveness.insert("orion".to_owned(), MachineLiveness::Offline);
    app
}

#[test]
fn session_list_machine_column() {
    let mut app = app_with_machines();
    app.config.prefs.machine_column = true;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_grouped_by_machine() {
    let mut app = app_with_machines();
    app.list_shape.group_by = crate::app::list_view::GroupBy::Machine;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_grouped_by_machine_after_one_goes_offline() {
    use cctui_proto::models::MachineLiveness;
    let mut app = app_with_machines();
    app.list_shape.group_by = crate::app::list_view::GroupBy::Machine;
    // The WS event is all it takes; no refetch stands between it and the header.
    reduce(
        &mut app,
        Action::SessionLive(crate::app::session_live::SessionLiveAction::MachineLiveness {
            machine_id: "cyberia-1".to_owned(),
            liveness: MachineLiveness::Offline,
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

fn app_with_stats() -> crate::app::App {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    app.machine_liveness.insert("orion".to_owned(), cctui_proto::models::MachineLiveness::Online);
    app.machine_liveness.insert("rigel".to_owned(), cctui_proto::models::MachineLiveness::Offline);
    reduce(
        &mut app,
        Action::Slice(SliceAction::StatsLoaded(Box::new(cctui_proto::api::SessionStats {
            total: 18,
            live: 3,
            needs_input: 1,
            archived: 6,
            today: 4,
            yesterday: 2,
            week: 11,
            month: 17,
        }))),
    );
    app
}

#[test]
fn session_list_tab_bar_and_summary() {
    let mut app = app_with_stats();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_summary_before_the_stats_land() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn overview_view() {
    let mut app = app_with_stats();
    reduce(&mut app, Action::Slice(SliceAction::Switch(3)));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn overview_view_before_the_stats_land() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    reduce(&mut app, Action::Slice(SliceAction::Switch(3)));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn overview_view_with_a_session_needing_input() {
    let mut app = app_with_stats();
    session_mut(&mut app, "s-blocked").attention = Some(cctui_proto::models::Attention::NeedsInput);
    reduce(&mut app, Action::Slice(SliceAction::Switch(3)));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn overview_view_narrow() {
    let mut app = app_with_stats();
    reduce(&mut app, Action::Slice(SliceAction::Switch(3)));
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}
