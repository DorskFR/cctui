use crate::app::View;
use crate::testsupport::{
    app_with_sessions, conversation_store, edit_permission_request, ended_session,
    permission_request, render_screen, render_screen_sized,
};

fn conversation_app() -> crate::app::App {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    app
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

#[test]
fn conversation() {
    let mut app = conversation_app();
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
    let mut app = conversation_app();
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

#[test]
fn help_overlay() {
    let mut app = app_with_sessions();
    app.router.push(View::Help);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_permission_card() {
    let mut app = conversation_app();
    app.permissions.push(permission_request());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_permission_card_with_a_diff() {
    let mut app = conversation_app();
    app.permissions.push(edit_permission_request());
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_two_permission_cards_stack() {
    let mut app = conversation_app();
    app.permissions.push(permission_request());
    app.permissions.push(edit_permission_request());
    insta::assert_snapshot!(render_screen(&mut app));
}

/// The card belongs to another session: this one shows only the status-bar
/// indicator, and its composer keeps every keystroke.
#[test]
fn conversation_pending_elsewhere_only_shows_the_indicator() {
    let mut app = conversation_app();
    let mut elsewhere = permission_request();
    elsewhere.session_id = "s-blocked".to_owned();
    app.permissions.push(elsewhere);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_permission_card_narrow() {
    let mut app = conversation_app();
    app.permissions.push(permission_request());
    insta::assert_snapshot!(render_screen_sized(&mut app, 60, 20));
}

#[test]
fn conversation_banner_working() {
    let mut app = conversation_app();
    app.clock_ms = 120_000;
    app.sessions[0].activity_detail = Some("running the tests".to_owned());
    app.sessions[0].last_activity_at = chrono::DateTime::from_timestamp_millis(105_000);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_banner_silent() {
    let mut app = conversation_app();
    app.clock_ms = 600_000;
    app.sessions[0].last_activity_at = chrono::DateTime::from_timestamp_millis(120_000);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_banner_waiting() {
    let mut app = conversation_app();
    app.sessions[0].bucket = cctui_proto::classifier::Bucket::Blocked;
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_ended_closes_the_composer() {
    let mut app = app_with_sessions();
    app.sessions[0] = ended_session("s-working", "cctui", "crashed", Some("exit status 139"));
    let id = app.sessions[0].id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_ended_failed_start_carries_its_detail() {
    let mut app = app_with_sessions();
    app.sessions[0] =
        ended_session("s-working", "cctui", "spawn_failed", Some("unknown model gpt-nope"));
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen(&mut app));
}
