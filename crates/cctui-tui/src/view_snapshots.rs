use crate::app::harness_mode::HarnessModeAction;
use cctui_proto::drafts::{Draft, DraftList, session_history_key};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::admin::AccessAction;
use crate::app::attach::AttachAction;
use crate::app::bookmarks::BookmarkAction;
use crate::app::cmdline::{CmdAction, Mode as CmdMode};
use crate::app::controls::{ControlsAction, PickerColumn};
use crate::app::diagnose::DiagnoseAction;
use crate::app::dispatchers::DispatcherAction;
use crate::app::drafts::DraftAction;
use crate::app::fileview::FileViewAction;
use crate::app::instance::InstanceAction;
use crate::app::labels::LabelAction;
use crate::app::list_search::ListSearchAction;
use crate::app::list_shape_reduce::ListShapeAction;
use crate::app::machines::MachineAction;
use crate::app::macros::MacroAction;
use crate::app::pins::PinAction;
use crate::app::sidebar::SidebarAction;
use crate::app::slice::SliceAction;
use crate::app::spend::SpendAction;
use crate::app::unread::UnreadAction;
use crate::app::usage::UsageAction;
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

fn spawn_dialog() -> crate::app::App {
    let mut app = app_with_sessions();
    reduce(&mut app, Action::Spawn(crate::app::spawn::SpawnAction::Open));
    app
}

#[test]
fn spawn_dialog_core_fields() {
    let mut app = spawn_dialog();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn spawn_dialog_with_a_git_badge_and_the_recent_dirs_open() {
    use crate::app::spawn::SpawnAction;

    let mut app = spawn_dialog();
    reduce(
        &mut app,
        Action::Spawn(SpawnAction::RecentDirsLoaded(vec![
            "/home/dev/cctui".to_owned(),
            "/home/dev/cctui-wt/cct-1101".to_owned(),
        ])),
    );
    let info = cctui_proto::git::GitInfo {
        is_repo: true,
        branch: Some("main".to_owned()),
        is_worktree: true,
        ..cctui_proto::git::GitInfo::default()
    };
    app.spawn.as_mut().expect("a form").cwd.asked =
        Some(("orion".to_owned(), "/home/dev/alpha".to_owned()));
    reduce(
        &mut app,
        Action::Spawn(SpawnAction::GitInfo {
            machine_id: "orion".to_owned(),
            path: "/home/dev/alpha".to_owned(),
            info: Some(Box::new(info)),
        }),
    );
    reduce(&mut app, Action::Spawn(SpawnAction::NextField));
    reduce(&mut app, Action::Spawn(SpawnAction::DirPick(1)));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn spawn_dialog_codex_shows_the_service_tier_and_a_mode_hint() {
    let mut app = spawn_dialog();
    let form = app.spawn.as_mut().expect("a form");
    form.fields.adapter_id = "codex".to_owned();
    form.fields.permission_mode = "yolo".to_owned();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn spawn_dialog_annotates_a_model_its_codex_is_too_old_for() {
    use cctui_proto::harness_models::{HarnessModels, ModelHint, ModelOption};

    let mut app = spawn_dialog();
    let form = app.spawn.as_mut().expect("a form");
    form.fields.adapter_id = "codex".to_owned();
    form.fields.model_codex = "gpt-6-preview".to_owned();
    form.models = Some(Box::new(HarnessModels {
        harness: "codex".to_owned(),
        models: vec![ModelOption {
            v: "gpt-6-preview".to_owned(),
            label: "GPT-6 preview".to_owned(),
            hint: Some(ModelHint::Gated {
                version: "0.200.0".to_owned(),
                current: "0.150.0".to_owned(),
            }),
            disabled: true,
        }],
        efforts: vec![String::new(), "high".to_owned()],
    }));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn spawn_dialog_reports_a_failed_launch_inline() {
    let mut app = spawn_dialog();
    app.spawn.as_mut().expect("a form").fields.working_dir.clear();
    reduce(&mut app, Action::Spawn(crate::app::spawn::SpawnAction::Submit));
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
/// The account, pool and usage payloads the spawn picker reads, as the server
/// sends them.
fn spawn_catalogs() -> Vec<crate::app::spawn::SpawnFetch> {
    use crate::app::spawn::SpawnFetch;
    vec![
        SpawnFetch::Accounts(
            serde_json::from_value(serde_json::json!([
                {
                    "id": U_ALICE, "name": "personal-max", "user_id": U_POOL,
                    "providers": [{
                        "id": "cred-a", "provider": "anthropic", "family": "anthropic",
                        "managed": false, "needs_reauth": false,
                    }],
                    "pool_eligible": true, "pool_weight": 1.0,
                },
                {
                    "id": U_BOB, "name": "work-team", "user_id": U_POOL,
                    "providers": [{
                        "id": "cred-b", "provider": "openai", "family": "openai",
                        "managed": false, "needs_reauth": false,
                    }],
                    "pool_eligible": true, "pool_weight": 1.0,
                },
            ]))
            .expect("accounts"),
        ),
        SpawnFetch::Pools(
            serde_json::from_value(serde_json::json!([
                {
                    "id": "p1", "user_id": U_POOL, "name": "personal",
                    "strategy": "ordered", "failover": true,
                    "members": [{
                        "account_id": U_ALICE, "name": "personal-max",
                        "position": 0, "owned": true, "pool_eligible": true,
                    }],
                },
            ]))
            .expect("pools"),
        ),
        SpawnFetch::Usage(
            serde_json::from_value(serde_json::json!([
                {
                    "account_id": U_ALICE, "account": U_ALICE,
                    "windows": [{"key": "session", "utilization": 62.0}],
                },
            ]))
            .expect("usage"),
        ),
    ]
}

#[test]
fn spawn_dialog_account_labels_env_files() {
    use crate::app::spawn::SpawnAction;
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

    for fetch in spawn_catalogs() {
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

    // Tab off the rows before the account picker, then pick the anthropic
    // account; the openai one is filtered out under claude-code.
    // Nothing the Dir row can complete, so Tab walks past it rather than
    // asking the machine again.
    app.spawn.as_mut().expect("the dialog is open").cwd.completions = vec!["/nowhere".to_owned()];
    let account = {
        let form = app.spawn.as_ref().expect("the dialog is open");
        form.sections.iter().position(|s| s.title() == "Account").expect("an account section")
    };
    for _ in 0..64 {
        if app.spawn.as_ref().expect("the dialog is open").focus.section == account {
            break;
        }
        tab(&mut app);
    }
    assert_eq!(
        app.spawn.as_ref().expect("the dialog is open").focus.section,
        account,
        "Tab reaches the account picker"
    );
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

#[test]
fn file_viewer_image_placeholder_with_dimensions() {
    let mut app = app_viewing("shot.png", "image/png", &tiny_png());
    insta::assert_snapshot!(render_screen(&mut app));
}

/// A real 8x8 PNG, so the chip shows measured dimensions. The snapshot is the
/// text fallback: no graphics protocol was ever queried.
fn tiny_png() -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(8, 8))
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("a png");
    out.into_inner()
}

#[test]
fn conversation_image_lines() {
    use crate::app::ConversationStore;
    use crate::app::line::agent_event_to_line;

    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    let mut store = ConversationStore::new();
    for (seq, body) in [
        (1, "▷ User: look at ![shot.png](cctui-img://img-1)"),
        (2, "![diagram.png](cctui-img://img-2)"),
    ] {
        let event = cctui_proto::ws::AgentEvent::Text {
            content: body.to_owned(),
            meta: false,
            kind: None,
            operation: None,
            ts: 0,
            message_id: None,
            usage: None,
            seq: Some(seq),
            turn_id: None,
        };
        store.push_live(Some(seq), agent_event_to_line(&event).expect("a line"));
    }
    app.conversations.insert(id, store);
    app.router.push(View::Conversation);
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

// --- Machines and dispatchers ---

fn machine_row(
    id: &str,
    name: &str,
    tier: cctui_proto::models::MachineLiveness,
    cpu: Option<f32>,
) -> cctui_client::MachineResourcesRow {
    cctui_client::MachineResourcesRow {
        machine_id: uuid::Uuid::parse_str(id).expect("a uuid"),
        name: name.to_owned(),
        display_name: None,
        hue: None,
        liveness: tier,
        last_seen_at: ms_ago(2_000),
        resources: cpu.map(|cpu_pct| cctui_proto::resources::MachineResources {
            cpu_pct,
            mem_pct: 20.0,
            mem_used_bytes: 12 << 30,
            mem_total_bytes: 64 << 30,
            disk_pct: 10.0,
            disk_used_bytes: 0,
            disk_total_bytes: 0,
            disk_path: String::new(),
            load1: None,
        }),
        updated_at: None,
    }
}

const M_A: &str = "11111111-1111-4111-8111-111111111111";
const M_B: &str = "22222222-2222-4222-8222-222222222222";
const M_C: &str = "33333333-3333-4333-8333-333333333333";

/// One machine of each tier, with a session running on the live one.
fn app_in_machines_slice() -> crate::app::App {
    use cctui_proto::models::MachineLiveness;
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    session_mut(&mut app, "s-working").machine_id = M_A.to_owned();
    let _ = reduce(
        &mut app,
        Action::Machines(MachineAction::Loaded(vec![
            machine_row(M_A, "cyberia-ws", MachineLiveness::Online, Some(31.4)),
            machine_row(M_B, "macbook", MachineLiveness::Stale, None),
            machine_row(M_C, "k3s-worker-2", MachineLiveness::Offline, None),
        ])),
    );
    let _ = reduce(&mut app, Action::Machines(MachineAction::Open));
    app
}

#[test]
fn machines_table() {
    let mut app = app_in_machines_slice();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn machines_table_at_eighty_columns() {
    let mut app = app_in_machines_slice();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn machines_table_after_one_comes_online() {
    use cctui_proto::models::MachineLiveness;
    let mut app = app_in_machines_slice();
    reduce(
        &mut app,
        Action::SessionLive(crate::app::session_live::SessionLiveAction::MachineLiveness {
            machine_id: M_C.to_owned(),
            liveness: MachineLiveness::Online,
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn machines_table_when_the_key_may_not_list_them() {
    let mut app = app_with_sessions();
    let _ = reduce(&mut app, Action::Machines(MachineAction::Open));
    let _ = reduce(
        &mut app,
        Action::Machines(MachineAction::Failed("this key may not list machines".to_owned())),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

fn dispatcher_row(
    id: &str,
    name: &str,
    kind: &str,
    connected: bool,
    tier: cctui_proto::models::MachineLiveness,
    pool: Option<&str>,
    account: Option<&str>,
) -> cctui_client::Dispatcher {
    cctui_client::Dispatcher {
        id: id.to_owned(),
        name: name.to_owned(),
        kind: kind.to_owned(),
        liveness: tier,
        connected,
        last_seen_at: ms_ago(2_000),
        default_account: account.map(str::to_owned),
        default_pool: pool.map(str::to_owned),
    }
}

fn app_with_dispatchers(scopes: &[&str]) -> crate::app::App {
    use cctui_proto::models::MachineLiveness;
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    app.auth = crate::app::identity::AuthState::Identified(crate::app::identity::Identity {
        role: "user".to_owned(),
        user_name: Some("dev".to_owned()),
        scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        token_preview: "abcd".to_owned(),
    });
    let _ = reduce(&mut app, Action::Dispatchers(DispatcherAction::Open));
    let _ = reduce(
        &mut app,
        Action::Dispatchers(DispatcherAction::Loaded(vec![
            dispatcher_row(
                "d-1",
                "k8s-cyberia",
                "kubernetes",
                true,
                MachineLiveness::Online,
                Some("work"),
                None,
            ),
            dispatcher_row(
                "d-2",
                "docker-mac",
                "docker",
                false,
                MachineLiveness::Stale,
                None,
                Some("personal-max"),
            ),
        ])),
    );
    app
}

#[test]
fn dispatchers_panel() {
    let mut app = app_with_dispatchers(&["enroll"]);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn dispatchers_panel_read_only_without_the_enroll_scope() {
    let mut app = app_with_dispatchers(&["sessions:write"]);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn dispatchers_enroll_form() {
    let mut app = app_with_dispatchers(&["enroll"]);
    let _ = reduce(&mut app, Action::Dispatchers(DispatcherAction::StartEnroll));
    for c in "k8s-tokyo".chars() {
        let _ = reduce(
            &mut app,
            Action::Dispatchers(DispatcherAction::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::NONE,
            ))),
        );
    }
    let _ = reduce(&mut app, Action::Dispatchers(DispatcherAction::NextField));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn dispatchers_one_shot_key_dialog() {
    let mut app = app_with_dispatchers(&["enroll"]);
    let _ = reduce(
        &mut app,
        Action::Dispatchers(DispatcherAction::Enrolled {
            name: "k8s-tokyo".to_owned(),
            reply: Box::new(cctui_client::EnrolledDispatcher {
                dispatcher_id: "d-9".to_owned(),
                dispatcher_key: "cctui-disp-EXAMPLEKEY0123456789".to_owned(),
            }),
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn dispatchers_delete_confirmation() {
    let mut app = app_with_dispatchers(&["enroll"]);
    let _ = reduce(&mut app, Action::Dispatchers(DispatcherAction::StartDelete));
    insta::assert_snapshot!(render_screen(&mut app));
}

// --- Foreign jobs and the harness mode ---

fn app_with_a_foreign_job() -> crate::app::App {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    session_mut(&mut app, "s-working").origin = cctui_proto::api::SessionOrigin::Foreign;
    app
}

#[test]
fn session_list_marks_a_job_cctui_did_not_start() {
    let mut app = app_with_a_foreign_job();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_marks_a_foreign_job_at_eighty_columns() {
    let mut app = app_with_a_foreign_job();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn foreign_job_archive_asks_before_removing_it_on_the_machine() {
    let mut app = app_with_a_foreign_job();
    focus(&mut app, "s-working");
    reduce(&mut app, Action::RowAction(crate::app::row_actions::RowAction::ArchiveOrUnarchive));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_info_popup_names_a_foreign_origin() {
    let mut app = app_with_a_foreign_job();
    focus(&mut app, "s-working");
    reduce(
        &mut app,
        Action::Diagnose(DiagnoseAction::Open(crate::app::diagnose::DiagnoseMode::Info)),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn harness_mode_picker() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    app.settings_blob = Some(serde_json::json!({"harnessMode": "bg"}));
    reduce(&mut app, Action::HarnessMode(HarnessModeAction::Open));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn harness_mode_picker_on_a_mode_that_is_not_in_use() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    app.settings_blob = Some(serde_json::json!({"harnessMode": "sdk"}));
    reduce(&mut app, Action::HarnessMode(HarnessModeAction::Open));
    reduce(&mut app, Action::HarnessMode(HarnessModeAction::SelectNext));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn harness_mode_picker_narrow() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    reduce(&mut app, Action::HarnessMode(HarnessModeAction::Open));
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

// --- Usage ---

const U_ALICE: &str = "aaaaaaaa-1111-4111-8111-111111111111";
const U_BOB: &str = "bbbbbbbb-2222-4222-8222-222222222222";
const U_POOL: &str = "cccccccc-3333-4333-8333-333333333333";

fn usage_window(
    key: &str,
    label: &str,
    utilization: Option<f64>,
    amount_usd: Option<f64>,
    resets_in_ms: Option<i64>,
    ratio: Option<f64>,
) -> cctui_client::UsageWindowView {
    cctui_client::UsageWindowView {
        key: key.to_owned(),
        kind: key.to_owned(),
        label: label.to_owned(),
        utilization,
        amount_usd,
        resets_at: resets_in_ms.map(|ms| ms_ago(-ms)),
        model_id: None,
        model_display_name: None,
        pace: ratio.map(|ratio| cctui_client::UsagePace {
            elapsed_fraction: 0.5,
            expected_pct: 50.0,
            ratio,
            projected_wall_at: None,
            slope_hours: None,
        }),
    }
}

fn usage_entry(
    account: &str,
    name: &str,
    provider: &str,
    windows: Vec<cctui_client::UsageWindowView>,
) -> cctui_client::AccountUsageEntry {
    cctui_client::AccountUsageEntry {
        account_id: uuid::Uuid::parse_str(account).expect("a uuid"),
        provider: provider.to_owned(),
        windows,
        age_secs: 0,
        account: uuid::Uuid::parse_str(account).expect("a uuid"),
        account_name: name.to_owned(),
        account_emoji: None,
        header_pin: true,
        ..cctui_client::AccountUsageEntry::default()
    }
}

/// A percent window burning hot, a dollar window and an unreported one — the
/// three readouts the panel has to tell apart.
fn app_in_usage_panel() -> crate::app::App {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    let _ = reduce(&mut app, Action::Usage(UsageAction::Open));
    let _ = reduce(
        &mut app,
        Action::Usage(UsageAction::PoolsLoaded(vec![cctui_client::PoolUsageView {
            pool_id: uuid::Uuid::parse_str(U_POOL).expect("a uuid"),
            name: "default".to_owned(),
            strategy: "headroom".to_owned(),
            failover: true,
            families: vec![
                cctui_client::PoolFamilyUsage {
                    family: "anthropic".to_owned(),
                    members: vec![cctui_client::PoolUsageMember {
                        account_id: uuid::Uuid::parse_str(U_ALICE).expect("a uuid"),
                        name: "alice".to_owned(),
                        emoji: None,
                        weight: 1.0,
                        usage_known: true,
                    }],
                    windows: vec![
                        cctui_client::PoolUsageWindow {
                            key: "session".to_owned(),
                            kind: "session".to_owned(),
                            label: "5h".to_owned(),
                            model_display_name: None,
                            level_pct: Some(78.0),
                            expected_pct: 55.0,
                            ratio: Some(1.4),
                            next_reset_at: Some(ms_ago(-72 * 60_000)),
                            projection: None,
                            projection_unavailable: None,
                        },
                        cctui_client::PoolUsageWindow {
                            key: "weekly_all".to_owned(),
                            kind: "weekly_all".to_owned(),
                            label: "7d".to_owned(),
                            model_display_name: None,
                            level_pct: Some(31.0),
                            expected_pct: 40.0,
                            ratio: None,
                            next_reset_at: Some(ms_ago(-3 * 24 * 3_600_000)),
                            projection: None,
                            projection_unavailable: None,
                        },
                    ],
                },
                cctui_client::PoolFamilyUsage {
                    family: "openai".to_owned(),
                    members: vec![cctui_client::PoolUsageMember {
                        account_id: uuid::Uuid::parse_str(U_BOB).expect("a uuid"),
                        name: "bob".to_owned(),
                        emoji: None,
                        weight: 1.0,
                        usage_known: false,
                    }],
                    windows: vec![cctui_client::PoolUsageWindow {
                        key: "session".to_owned(),
                        kind: "session".to_owned(),
                        label: "5h".to_owned(),
                        model_display_name: None,
                        level_pct: None,
                        expected_pct: 0.0,
                        ratio: None,
                        next_reset_at: None,
                        projection: None,
                        projection_unavailable: Some("too young to rate".to_owned()),
                    }],
                },
            ],
        }])),
    );
    let _ = reduce(
        &mut app,
        Action::Usage(UsageAction::AccountsLoaded(vec![
            usage_entry(
                U_ALICE,
                "alice",
                "anthropic",
                vec![
                    usage_window("session", "5h", Some(91.0), None, Some(22 * 60_000), Some(1.8)),
                    usage_window("weekly_all", "7d", None, None, None, None),
                ],
            ),
            usage_entry(
                U_BOB,
                "bob",
                "openai",
                vec![usage_window("usd_5h", "$", None, Some(12.4), None, None)],
            ),
        ])),
    );
    app
}

#[test]
fn usage_panel_percent_dollar_and_unreported_windows() {
    let mut app = app_in_usage_panel();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn usage_panel_at_eighty_columns() {
    let mut app = app_in_usage_panel();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn usage_panel_with_the_accounts_pane_focused() {
    let mut app = app_in_usage_panel();
    reduce(&mut app, Action::Usage(UsageAction::SwitchPane));
    reduce(&mut app, Action::Usage(UsageAction::SelectNext));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn usage_panel_when_the_key_may_not_read_usage() {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    reduce(&mut app, Action::Usage(UsageAction::Open));
    reduce(&mut app, Action::Usage(UsageAction::Failed("this key may not read usage".to_owned())));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn the_status_line_carries_the_worst_window_of_each_family() {
    let mut app = app_in_usage_panel();
    reduce(&mut app, Action::Usage(UsageAction::Close));
    insta::assert_snapshot!(render_screen_sized(&mut app, 100, 12));
}

fn app_with_account_picker() -> crate::app::state::App {
    use crate::app::account_switch::AccountSwitchAction;
    use cctui_clientcore::account_switch::{Binding, Credential, Window};

    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    reduce(&mut app, Action::AccountSwitch(AccountSwitchAction::Open));
    let cred = |name: &str, provider: &str, pct: f64, resets: i64| Credential {
        account_id: format!("{name}-id"),
        account_name: name.to_owned(),
        provider: provider.to_owned(),
        windows: vec![Window { pct, resets_in_secs: Some(resets) }],
    };
    reduce(
        &mut app,
        Action::AccountSwitch(AccountSwitchAction::Loaded {
            bindings: vec![
                Binding {
                    family: "anthropic".to_owned(),
                    account_id: "alice@max-id".to_owned(),
                    account_name: "alice@max".to_owned(),
                },
                Binding {
                    family: "openai".to_owned(),
                    account_id: "oai-id".to_owned(),
                    account_name: "oai".to_owned(),
                },
            ],
            credentials: vec![
                cred("alice@max", "anthropic", 91.0, 18_000),
                cred("bob@max", "anthropic", 99.0, 900),
                cred("carol@max", "anthropic", 12.0, 18_000),
                cred("oai", "openai", 5.0, 600),
                cred("oai-spare", "openai", 7.0, 600),
            ],
        }),
    );
    app
}

#[test]
fn account_switch_picker() {
    let mut app = app_with_account_picker();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn account_switch_picker_on_the_other_family() {
    use crate::app::account_switch::AccountSwitchAction;
    let mut app = app_with_account_picker();
    reduce(&mut app, Action::AccountSwitch(AccountSwitchAction::NextBinding));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn account_switch_picker_while_loading() {
    use crate::app::account_switch::AccountSwitchAction;
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    reduce(&mut app, Action::AccountSwitch(AccountSwitchAction::Open));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn account_switch_picker_narrow() {
    let mut app = app_with_account_picker();
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

// --- Spend ---

/// Dollars, token windows and a sparkline, all anchored to the test clock's own
/// local midnight so the bars land on the same slots in every timezone.
fn app_in_spend_slice() -> crate::app::App {
    use cctui_proto::api::{TokenUsageWindows, UsageAnalytics, UsageBucket, WindowTokenUsage};

    const fn window(input: u64, output: u64, cache_read: u64) -> WindowTokenUsage {
        WindowTokenUsage { input, output, cache_read }
    }

    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;

    let today = session_mut(&mut app, "s-working");
    today.model = Some("claude-opus-5-20260101".to_owned());
    today.registered_at = Some(chrono::DateTime::from_timestamp_millis(CLOCK_MS).expect("a stamp"));
    today.token_usage.cost_usd = 4.10;

    let older = session_mut(&mut app, "s-done");
    older.model = Some("claude-sonnet-5".to_owned());
    older.registered_at = Some(ms_ago(10 * 86_400_000));
    older.token_usage.cost_usd = 22.40;

    let midnight = crate::app::spend::local_midnight_ms(CLOCK_MS).expect("a local midnight");
    let day = |back: i64| {
        chrono::DateTime::from_timestamp_millis(midnight - back * 86_400_000)
            .expect("a stamp")
            .to_rfc3339()
    };
    let bucket = |back: i64, output: u64| UsageBucket {
        bucket: day(back),
        input: output / 2,
        output,
        cache_read: 0,
        cache_creation: 0,
    };

    let _ = reduce(
        &mut app,
        Action::Spend(SpendAction::Loaded(Box::new(crate::app::spend::SpendData {
            windows: TokenUsageWindows {
                hour: window(12_000, 3_400, 180_000),
                today: window(210_000, 48_000, 1_900_000),
                day: window(480_000, 96_000, 4_200_000),
                week: window(3_100_000, 640_000, 29_000_000),
                month: window(12_400_000, 2_600_000, 118_000_000),
            },
            analytics: UsageAnalytics {
                granularity: "day".to_owned(),
                buckets: vec![
                    bucket(6, 900_000),
                    bucket(4, 2_400_000),
                    bucket(2, 1_300_000),
                    bucket(1, 3_800_000),
                    bucket(0, 1_700_000),
                ],
                models: Vec::new(),
                heatmap: Vec::new(),
            },
            cache_loss: vec![cctui_proto::api::cache_loss::DailyCacheLoss {
                day: "2023-11-13".to_owned(),
                ttl_expired: 1.25,
                gateway_rewrote_body: 0.4,
                unknown: 0.0,
                total: 1.65,
                ttl_expired_tokens: 820_000,
                gateway_rewrote_body_tokens: 260_000,
                unknown_tokens: 0,
                lost_tokens: 1_080_000,
                busts: 4,
            }],
        }))),
    );
    let _ = reduce(&mut app, Action::Spend(SpendAction::Open));
    app
}

#[test]
fn spend_panel() {
    let mut app = app_in_spend_slice();
    insta::assert_snapshot!(render_screen(&mut app));
}

// -- the dispatch tab, as a section of the spawn dialog --

/// The dialog open on the Dispatch tab with a job filled in. The dispatcher
/// names arrive the way every catalog does: through `SpawnData`.
fn app_dispatching() -> crate::app::App {
    use crate::app::dispatch::Field;
    use crate::app::spawn::{SpawnAction, SpawnFetch, SpawnTarget};

    let mut app = app_with_sessions();
    reduce(&mut app, Action::Spawn(SpawnAction::Open));
    reduce(
        &mut app,
        Action::Spawn(SpawnAction::DataLoaded(Box::new(SpawnFetch::Dispatchers(vec![
            "k8s-cyberia".to_owned(),
            "docker-local".to_owned(),
        ])))),
    );
    if let Some(form) = app.spawn.as_mut() {
        form.target = SpawnTarget::Dispatch;
    }
    for (field, text) in [
        (Field::Repo, "cctui"),
        (Field::Ticket, "CCT-1102"),
        (Field::Timeout, "60"),
        (Field::PackUrl, "https://git.example/packs.git"),
        (Field::PackRef, "main"),
        (Field::PackSubdir, "packs/cctui"),
        (Field::PackToken, "hunter2"),
    ] {
        let row = Field::ORDER.iter().position(|f| *f == field).expect("a field");
        for c in text.chars() {
            dispatch_key(&mut app, row, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }
    app
}

/// A key straight at the section, so the fixture does not depend on where the
/// dialog's global focus happens to sit.
fn dispatch_key(app: &mut crate::app::App, row: usize, key: KeyEvent) {
    let Some(form) = app.spawn.as_mut() else { return };
    let mut fields = form.fields.clone();
    for section in &mut form.sections {
        if section.title() == "Dispatch" {
            let _ = section.handle(row, key, &mut fields);
        }
    }
    form.fields = fields;
}

#[test]
fn spawn_dispatch_tab() {
    let mut app = app_dispatching();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn spend_panel_at_eighty_columns() {
    let mut app = app_in_spend_slice();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn spend_panel_when_the_key_may_not_read_stats() {
    let mut app = app_with_sessions();
    let _ = reduce(&mut app, Action::Spend(SpendAction::Open));
    let _ = reduce(
        &mut app,
        Action::Spend(SpendAction::Failed("this key may not read usage stats".to_owned())),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

/// No priced session in the range: the table says so rather than showing zeroes.
#[test]
fn spend_panel_without_priced_sessions() {
    let mut app = app_in_spend_slice();
    for s in &mut app.sessions {
        s.token_usage.cost_usd = 0.0;
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_header_carries_the_langfuse_cost() {
    let mut app = app_in_conversation();
    let id = app.selected_session().expect("a selected session").id.clone();
    reduce(
        &mut app,
        Action::Spend(SpendAction::Langfuse {
            session_id: id,
            usage: Some(cctui_clientcore::spend::LangfuseSpend {
                cost_usd: 0.734,
                trace_count: 12,
            }),
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

/// A deployment with no Langfuse sink answers nothing: the header is unchanged.
#[test]
fn conversation_header_without_langfuse_is_unchanged() {
    let mut app = app_in_conversation();
    let id = app.selected_session().expect("a selected session").id.clone();
    reduce(&mut app, Action::Spend(SpendAction::Langfuse { session_id: id, usage: None }));
    insta::assert_snapshot!(render_screen(&mut app));
}

// --- Instance status and the server's self-update ---

fn app_with_instance(role: &str, latest: Option<&str>, hook: bool, ready: bool) -> crate::app::App {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    app.auth = crate::app::identity::AuthState::Identified(crate::app::identity::Identity {
        role: role.to_owned(),
        user_name: Some("dev".to_owned()),
        scopes: vec!["read".to_owned()],
        token_preview: "abcd".to_owned(),
    });
    let _ = reduce(&mut app, Action::Instance(InstanceAction::Open));
    let _ = reduce(
        &mut app,
        Action::Instance(InstanceAction::Loaded(Box::new(cctui_client::VersionInfo {
            version: "0.23.0".to_owned(),
            git_hash: "deadbeef".to_owned(),
            commit_url: "https://example.invalid/commit/deadbeef".to_owned(),
            latest_version: latest.map(str::to_owned),
            latest_url: latest.map(|_| "https://example.invalid/release".to_owned()),
            instance_name: Some("cyberia".to_owned()),
            self_update_ready: ready,
            self_update_hook: hook,
        }))),
    );
    let _ = reduce(&mut app, Action::Instance(InstanceAction::RunLoaded(None)));
    app
}

fn hook_run(
    phase: cctui_proto::updatehook::UpdateHookPhase,
    done: bool,
    tail: Option<&str>,
) -> cctui_client::SelfUpdateRun {
    cctui_client::SelfUpdateRun {
        id: uuid::Uuid::nil(),
        version: "0.23.1".to_owned(),
        from_version: "0.23.0".to_owned(),
        phase,
        done,
        exit_code: if done { Some(i32::from(!phase.is_success())) } else { None },
        detail: "kubectl rollout restart deploy/cctui".to_owned(),
        output_tail: tail.map(str::to_owned),
        started_at: ms_ago(90_000),
        updated_at: ms_ago(2_000),
    }
}

#[test]
fn instance_panel_up_to_date() {
    let mut app = app_with_instance("admin", None, true, true);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn instance_panel_with_an_update_and_release_notes() {
    let mut app = app_with_instance("admin", Some("0.23.1"), true, true);
    let _ = reduce(
        &mut app,
        Action::Instance(InstanceAction::ChangelogLoaded(vec![cctui_client::ReleaseNote {
            version: "0.23.1".to_owned(),
            url: "https://example.invalid/release".to_owned(),
            body: "- fixed the reconnect watchdog\n- faster list paging".to_owned(),
            published_at: None,
        }])),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn instance_panel_without_a_configured_self_update_machine() {
    let mut app = app_with_instance("admin", Some("0.23.1"), false, false);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn instance_update_confirm_names_the_mechanism() {
    let mut app = app_with_instance("admin", Some("0.23.1"), true, true);
    let _ = reduce(&mut app, Action::Instance(InstanceAction::StartUpdate));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn instance_update_confirm_warns_when_an_agent_will_do_it() {
    let mut app = app_with_instance("admin", Some("0.23.1"), false, true);
    let _ = reduce(&mut app, Action::Instance(InstanceAction::StartUpdate));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn instance_panel_follows_a_hook_run() {
    let mut app = app_with_instance("admin", Some("0.23.1"), true, true);
    let _ = reduce(
        &mut app,
        Action::Instance(InstanceAction::RunLoaded(Some(Box::new(hook_run(
            cctui_proto::updatehook::UpdateHookPhase::Verifying,
            false,
            Some("deployment.apps/cctui restarted\nwaiting for rollout"),
        ))))),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn instance_panel_shows_a_rolled_back_run() {
    let mut app = app_with_instance("admin", Some("0.23.1"), true, true);
    let _ = reduce(
        &mut app,
        Action::Instance(InstanceAction::RunLoaded(Some(Box::new(hook_run(
            cctui_proto::updatehook::UpdateHookPhase::RolledBack,
            true,
            Some("health check never reported 0.23.1"),
        ))))),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

// --- Accounts and pools ---

fn account(id: &str, name: &str, families: &[&str]) -> cctui_client::Account {
    cctui_client::Account {
        id: id.to_owned(),
        name: name.to_owned(),
        emoji: None,
        user_id: "u1".to_owned(),
        user_name: None,
        providers: families
            .iter()
            .enumerate()
            .map(|(i, family)| cctui_client::AccountProvider {
                id: format!("{id}-p{i}"),
                provider: (*family).to_owned(),
                family: (*family).to_owned(),
                managed: false,
                needs_reauth: false,
                last_auth_error: None,
                est_cost_usd: 12.5,
                total_tokens: 931_000,
                last_used_at: None,
                header_pin: false,
            })
            .collect(),
        pool_eligible: true,
        pool_weight: 1.0,
    }
}

fn pool_member(id: &str, name: &str, position: i32) -> cctui_client::AccountPoolMember {
    cctui_client::AccountPoolMember {
        account_id: id.to_owned(),
        name: name.to_owned(),
        position,
        owned: true,
        pool_eligible: true,
    }
}

/// The usage reading is keyed on the account identity, so these rows carry real
/// uuids. `U_ALICE` / `U_BOB` are the usage fixture's own, reused so one usage
/// row describes the same account in both.
const ACCT_C: &str = "dddddddd-4444-4444-8444-444444444444";

fn app_in_accounts_slice() -> crate::app::App {
    use crate::app::accounts::AccountAction;
    use crate::app::pools::PoolAction;

    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    let mut withheld = account(U_BOB, "bob@max", &["anthropic"]);
    withheld.pool_eligible = false;
    withheld.pool_weight = 0.5;
    let _ = reduce(
        &mut app,
        Action::Accounts(AccountAction::Loaded(vec![
            account(U_ALICE, "alice@max", &["anthropic"]),
            withheld,
            account(ACCT_C, "ops-codex", &["openai"]),
        ])),
    );
    let _ = reduce(
        &mut app,
        Action::Accounts(AccountAction::RedirectsLoaded(vec![cctui_client::AccountRedirect {
            id: "r1".to_owned(),
            from_account: U_BOB.to_owned(),
            to_account: Some(U_ALICE.to_owned()),
            family: "anthropic".to_owned(),
            to_model: None,
            expires_at: None,
            reason: None,
        }])),
    );
    let _ = reduce(
        &mut app,
        Action::Pools(PoolAction::Loaded(vec![
            cctui_client::AccountPool {
                id: "p1".to_owned(),
                user_id: "u1".to_owned(),
                name: "default".to_owned(),
                strategy: "ordered".to_owned(),
                failover: true,
                members: vec![
                    pool_member(U_BOB, "bob@max", 1),
                    pool_member(U_ALICE, "alice@max", 0),
                ],
            },
            cctui_client::AccountPool {
                id: "p2".to_owned(),
                user_id: "u1".to_owned(),
                name: "codex".to_owned(),
                strategy: "headroom".to_owned(),
                failover: false,
                members: vec![pool_member(ACCT_C, "ops-codex", 0)],
            },
        ])),
    );
    app.usage.accounts = vec![usage_entry(
        U_ALICE,
        "alice@max",
        "anthropic",
        vec![usage_window("five_hour", "5h", Some(91.0), None, Some(22 * 60 * 1000), None)],
    )];
    let _ = reduce(&mut app, Action::Accounts(AccountAction::Open));
    app
}

#[test]
fn spawn_dispatch_tab_codex_harness() {
    let mut app = app_dispatching();
    dispatch_key(&mut app, 0, KeyEvent::new(KeyCode::Char('h'), KeyModifiers::CONTROL));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn spawn_dispatch_tab_at_eighty_columns() {
    let mut app = app_dispatching();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

/// No dispatcher enrolled: the section takes no rows and draws nothing.
#[test]
fn spawn_dispatch_section_hidden_without_a_dispatcher() {
    use crate::app::spawn::SpawnAction;
    let mut app = app_with_sessions();
    reduce(&mut app, Action::Spawn(SpawnAction::Open));
    let rendered = render_screen(&mut app);
    assert!(!rendered.contains("Dispatcher"), "{rendered}");
    insta::assert_snapshot!(rendered);
}

fn app_forking(codex: bool) -> crate::app::App {
    use crate::app::forkform::ForkAction;
    use cctui_proto::harness_models::{HarnessModels, ModelOption};
    let mut app = app_on("s-working");
    if codex {
        session_mut(&mut app, "s-working").adapter_id =
            Some(cctui_proto::adapter::AdapterId::new("codex"));
    }
    session_mut(&mut app, "s-working").model = Some("opus".to_owned());
    session_mut(&mut app, "s-working").effort = Some("high".to_owned());
    focus(&mut app, "s-working");
    reduce(&mut app, Action::Fork(ForkAction::Open));
    reduce(
        &mut app,
        Action::Fork(ForkAction::ModelsLoaded(Box::new(HarnessModels {
            harness: "claude-code".to_owned(),
            models: vec![
                ModelOption {
                    v: String::new(),
                    label: "Default".to_owned(),
                    hint: None,
                    disabled: false,
                },
                ModelOption {
                    v: "sonnet".to_owned(),
                    label: "Sonnet".to_owned(),
                    hint: None,
                    disabled: false,
                },
            ],
            efforts: vec![String::new(), "low".to_owned(), "high".to_owned()],
        }))),
    );
    app
}

#[test]
fn accounts_table() {
    let mut app = app_in_accounts_slice();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn fork_dialog() {
    let mut app = app_forking(false);
    insta::assert_snapshot!(render_screen(&mut app));
}

/// Codex cannot fork a slice, so the row is not offered at all.
#[test]
fn fork_dialog_codex_hides_the_extract_row() {
    let mut app = app_forking(true);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn accounts_table_at_eighty_columns() {
    let mut app = app_in_accounts_slice();
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn accounts_detail_pane() {
    use crate::app::accounts::AccountAction;
    let mut app = app_in_accounts_slice();
    reduce(&mut app, Action::Accounts(AccountAction::ToggleDetail));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn accounts_pools_pane_focused() {
    use crate::app::accounts::AccountAction;
    let mut app = app_in_accounts_slice();
    reduce(&mut app, Action::Accounts(AccountAction::ToggleFocus));
    reduce(&mut app, Action::Pools(crate::app::pools::PoolAction::SelectNext));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn accounts_reset_confirm_names_the_credit() {
    use crate::app::accounts::AccountAction;
    let mut app = app_in_accounts_slice();
    let mut entry = usage_entry(
        U_ALICE,
        "alice@max",
        "anthropic",
        vec![usage_window("five_hour", "5h", Some(100.0), None, Some(3 * 60 * 1000), None)],
    );
    entry.limit_reset = Some(cctui_client::LimitResetStatusView {
        kind: "claude".to_owned(),
        available: true,
        title: Some("Full reset (Weekly + 5 hr)".to_owned()),
        credit_id: Some("c-7".to_owned()),
        ineligible_reason: None,
        next_available_at: None,
    });
    app.usage.accounts = vec![entry];
    reduce(&mut app, Action::Accounts(AccountAction::StartReset));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn accounts_redirect_picker() {
    use crate::app::accounts::AccountAction;
    let mut app = app_in_accounts_slice();
    reduce(&mut app, Action::Accounts(AccountAction::StartRedirect));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn accounts_new_pool_form() {
    use crate::app::pools::PoolAction;
    let mut app = app_in_accounts_slice();
    reduce(&mut app, Action::Accounts(crate::app::accounts::AccountAction::ToggleFocus));
    reduce(&mut app, Action::Pools(PoolAction::StartNew));
    for c in "overflow".chars() {
        reduce(
            &mut app,
            Action::Pools(PoolAction::Key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Char(c),
                crossterm::event::KeyModifiers::NONE,
            ))),
        );
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn fork_dialog_on_the_prompt_row() {
    use crate::app::forkform::ForkAction;
    let mut app = app_forking(false);
    for _ in 0..4 {
        reduce(&mut app, Action::Fork(ForkAction::FocusNext));
    }
    for c in "try the other approach".chars() {
        reduce(
            &mut app,
            Action::Fork(ForkAction::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))),
        );
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn accounts_pool_delete_needs_the_name_typed() {
    use crate::app::pools::PoolAction;
    let mut app = app_in_accounts_slice();
    reduce(&mut app, Action::Accounts(crate::app::accounts::AccountAction::ToggleFocus));
    reduce(&mut app, Action::Pools(PoolAction::StartDelete));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn accounts_when_the_key_may_not_list_them() {
    use crate::app::accounts::AccountAction;
    let mut app = app_with_sessions();
    let _ = reduce(&mut app, Action::Accounts(AccountAction::Open));
    let _ = reduce(
        &mut app,
        Action::Accounts(AccountAction::Failed("this key may not list accounts".to_owned())),
    );
    let _ = reduce(
        &mut app,
        Action::Pools(crate::app::pools::PoolAction::Failed(
            "this key may not list accounts".to_owned(),
        )),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

// --- Access (admin) ---

fn access_user(id: &str, name: &str, disabled: bool) -> cctui_client::User {
    cctui_client::User {
        id: id.to_owned(),
        name: name.to_owned(),
        created_at: ms_ago(90_000),
        revoked_at: None,
        disabled_at: disabled.then(|| ms_ago(5_000)),
        can_dispatch: !disabled,
        last_seen_at: Some(ms_ago(2_000)),
    }
}

fn app_in_access_slice(scopes: &[&str]) -> crate::app::App {
    let mut app = app_with_sessions();
    app.clock_ms = CLOCK_MS;
    app.auth = crate::app::identity::AuthState::Identified(crate::app::identity::Identity {
        role: "admin".to_owned(),
        user_name: Some("dorsk".to_owned()),
        scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        token_preview: "cctui_u_ab12…ef34".to_owned(),
    });
    let _ = reduce(&mut app, Action::Access(AccessAction::Open));
    let _ = reduce(
        &mut app,
        Action::Access(AccessAction::UsersLoaded(vec![
            access_user("u-1", "dorsk", false),
            access_user("u-2", "nanachi", true),
        ])),
    );
    let _ = reduce(
        &mut app,
        Action::Access(AccessAction::DetailLoaded {
            tokens: vec![cctui_client::UserToken {
                id: "t-1".to_owned(),
                label: Some("laptop".to_owned()),
                created_at: ms_ago(80_000),
                expires_at: None,
                revoked_at: None,
                token_preview: Some("cctui_u_ab12…ef34".to_owned()),
            }],
            machines: vec![cctui_client::UserMachine {
                id: "m-1".to_owned(),
                name: "cyberia-ws".to_owned(),
                display_name: None,
                last_seen_at: ms_ago(2_000),
                revoked_at: None,
                kind: "persistent".to_owned(),
                key_preview: Some("cctui_m_cd34…ab12".to_owned()),
                liveness: cctui_proto::models::MachineLiveness::Online,
            }],
            keys: vec![cctui_client::ApiKey {
                id: "k-1".to_owned(),
                label: Some("ci".to_owned()),
                key_preview: Some("cctui_k_ef56…7890".to_owned()),
                kind: "user".to_owned(),
                created_at: ms_ago(70_000),
                expires_at: None,
                revoked_at: None,
                last_used_at: Some(ms_ago(1_000)),
                scopes: vec!["read".to_owned()],
            }],
            ceiling: vec!["read".to_owned(), "dispatch".to_owned()],
        }),
    );
    app
}

#[test]
fn access_user_list() {
    let mut app = app_in_access_slice(&["admin"]);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn access_user_list_at_eighty_columns() {
    let mut app = app_in_access_slice(&["admin"]);
    insta::assert_snapshot!(render_screen_sized(&mut app, 80, 24));
}

#[test]
fn access_tokens_tab() {
    let mut app = app_in_access_slice(&["admin"]);
    let _ = reduce(&mut app, Action::Access(AccessAction::NextTab));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn access_machines_tab() {
    let mut app = app_in_access_slice(&["admin"]);
    for _ in 0..2 {
        let _ = reduce(&mut app, Action::Access(AccessAction::NextTab));
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn access_keys_tab() {
    let mut app = app_in_access_slice(&["admin"]);
    for _ in 0..3 {
        let _ = reduce(&mut app, Action::Access(AccessAction::NextTab));
    }
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn access_mint_key_dialog_marks_the_scopes_outside_the_ceiling() {
    let mut app = app_in_access_slice(&["admin"]);
    for _ in 0..3 {
        let _ = reduce(&mut app, Action::Access(AccessAction::NextTab));
    }
    let _ = reduce(&mut app, Action::Access(AccessAction::StartNew));
    let _ = reduce(&mut app, Action::Access(AccessAction::ToggleScope));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn access_purge_asks_for_the_name_to_be_typed() {
    let mut app = app_in_access_slice(&["admin"]);
    let _ = reduce(&mut app, Action::Access(AccessAction::StartPurge));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn access_revoke_asks_first() {
    let mut app = app_in_access_slice(&["admin"]);
    let _ = reduce(&mut app, Action::Access(AccessAction::StartRevoke));
    insta::assert_snapshot!(render_screen(&mut app));
}

/// A placeholder stands in for the secret so the snapshot file never holds a
/// credential-shaped string.
#[test]
fn access_shows_a_new_secret_once() {
    let mut app = app_in_access_slice(&["admin"]);
    let _ = reduce(
        &mut app,
        Action::Access(AccessAction::Secret {
            what: "api key",
            secret: "<the-new-key>".to_owned(),
        }),
    );
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn access_re_grant_dialog_prefills_what_the_key_already_holds() {
    let mut app = app_in_access_slice(&["admin"]);
    for _ in 0..3 {
        let _ = reduce(&mut app, Action::Access(AccessAction::NextTab));
    }
    let _ = reduce(&mut app, Action::Access(AccessAction::StartKeyScopes));
    insta::assert_snapshot!(render_screen(&mut app));
}
