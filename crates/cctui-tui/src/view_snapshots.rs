use crate::app::View;
use crate::testsupport::{
    app_with_sessions, ask_card, conversation_store, permission_request, plan_card, render_screen,
    render_screen_sized,
};

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

#[test]
fn conversation() {
    let mut app = app_with_sessions();
    let id = app.selected_session().expect("a selected session").id.clone();
    app.conversations.insert(id, conversation_store());
    app.router.push(View::Conversation);
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
