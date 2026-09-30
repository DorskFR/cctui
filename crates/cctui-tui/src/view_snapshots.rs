use crate::app::View;
use crate::testsupport::{
    app_with_sessions, conversation_lines, permission_request, render_screen, render_screen_sized,
};

#[test]
fn session_list() {
    let mut app = app_with_sessions();
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn session_list_truncated() {
    let mut app = app_with_sessions();
    for i in 0..4 {
        app.sessions.push(crate::testsupport::session(
            &format!("s-extra-{i}"),
            &format!("extra{i}"),
            "active",
            "working",
        ));
    }
    app.show_all_sessions = false;
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
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.stream_buffer.insert(id, conversation_lines());
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
    app.stream_buffer.insert(id, conversation_lines());
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
