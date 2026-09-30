use cctui_proto::drafts::{Draft, DraftList, session_history_key};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::drafts::DraftAction;
use crate::app::{Action, View, reduce};
use crate::testsupport::{
    app_with_sessions, conversation_store, permission_request, render_screen, render_screen_sized,
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

#[test]
fn conversation() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn conversation_with_an_unsent_draft() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
    app.input_active = true;
    let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
    reduce(&mut app, Action::InputKey(key));
    insta::assert_snapshot!(render_screen(&mut app));
}

#[test]
fn history_picker() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id.clone(), conversation_store());
    app.router.push(View::Conversation);
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
