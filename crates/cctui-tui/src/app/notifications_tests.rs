use cctui_proto::api::notifications::{Notification, NotificationList, NotificationTransition};
use cctui_proto::ws::ServerEvent;

use super::{NotificationAction, NotificationEffect, actions, reduce_notifications};
use crate::app::action::{Action, Effect};
use crate::app::state::{App, View};

fn at(secs: i64) -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(secs, 0).expect("a timestamp")
}

fn notice(n: u128, title: &str, state: &str) -> Notification {
    Notification {
        id: uuid::Uuid::from_u128(n),
        user_id: uuid::Uuid::from_u128(99),
        kind: "approval".to_owned(),
        severity: "high".to_owned(),
        session_id: Some("s-1".to_owned()),
        machine_id: None,
        subject_kind: None,
        subject_id: None,
        collapse_key: None,
        title: title.to_owned(),
        body: Some("ship it to prod".to_owned()),
        tags: None,
        actions: serde_json::json!([
            { "id": "approve", "label": "Approve" },
            { "id": "reject", "label": "Reject", "style": "danger" },
        ]),
        state: state.to_owned(),
        repeat_count: 1,
        decision: None,
        decision_note: None,
        acted_channel: None,
        acted_by: None,
        resolved_reason: None,
        created_at: at(i64::try_from(n).expect("small")),
        updated_at: at(0),
        read_at: None,
        acted_at: None,
        resolved_at: None,
        expires_at: None,
        anchor_seq: None,
    }
}

fn dispatch(app: &mut App, action: NotificationAction) -> Vec<Effect> {
    reduce_notifications(app, action)
}

fn loaded(app: &mut App, items: Vec<Notification>, unread: i64) -> Vec<Effect> {
    let list = NotificationList { notifications: items, unread, has_more: false };
    dispatch(app, NotificationAction::Loaded(Box::new(list)))
}

fn open_with(items: Vec<Notification>, unread: i64) -> (App, Vec<Effect>) {
    let mut app = App::new();
    let opened = dispatch(&mut app, NotificationAction::Open);
    assert!(matches!(opened.as_slice(), [Effect::Notifications(NotificationEffect::Fetch)]));
    let effects = loaded(&mut app, items, unread);
    (app, effects)
}

fn read_id(effects: &[Effect]) -> Option<String> {
    match effects {
        [Effect::Notifications(NotificationEffect::Read { id })] => Some(id.clone()),
        _ => None,
    }
}

fn titles(app: &App) -> Vec<&str> {
    app.notifications.items.iter().map(|n| n.title.as_str()).collect()
}

#[test]
fn a_frame_inserts_newest_first_and_replaces_by_id() {
    let mut app = App::new();
    dispatch(&mut app, NotificationAction::Upsert(Box::new(notice(1, "old", "delivered"))));
    dispatch(&mut app, NotificationAction::Upsert(Box::new(notice(2, "new", "delivered"))));
    assert_eq!(titles(&app), ["new", "old"]);

    dispatch(&mut app, NotificationAction::Upsert(Box::new(notice(1, "old, again", "read"))));
    assert_eq!(titles(&app), ["new", "old, again"]);
}

#[test]
fn a_terminal_state_drops_the_notice() {
    let mut app = App::new();
    dispatch(&mut app, NotificationAction::Upsert(Box::new(notice(1, "a", "delivered"))));
    dispatch(&mut app, NotificationAction::Upsert(Box::new(notice(2, "b", "acted"))));
    dispatch(&mut app, NotificationAction::Upsert(Box::new(notice(1, "a", "resolved"))));
    assert_eq!(titles(&app), ["b"], "acted stays listed, resolved leaves");
    dispatch(&mut app, NotificationAction::Upsert(Box::new(notice(2, "b", "expired"))));
    assert!(app.notifications.items.is_empty());
}

#[test]
fn the_unread_frame_sets_the_badge() {
    let mut app = App::new();
    dispatch(&mut app, NotificationAction::Unread(4));
    assert_eq!(app.notifications.unread, 4);
    loaded(&mut app, Vec::new(), 1);
    assert_eq!(app.notifications.unread, 1, "a list reply carries the count too");
}

#[test]
fn ws_frames_reach_the_reducer() {
    let n = notice(1, "a", "delivered");
    let upsert = crate::app::server_event::to_actions(ServerEvent::Notification {
        notification: Box::new(n),
    });
    assert!(matches!(
        upsert.as_slice(),
        [Action::Notifications(NotificationAction::Upsert(n))] if n.title == "a"
    ));
    let unread = crate::app::server_event::to_actions(ServerEvent::NotificationsUnread {
        user_id: uuid::Uuid::nil(),
        count: 3,
    });
    assert!(matches!(unread.as_slice(), [Action::Notifications(NotificationAction::Unread(3))]));
}

#[test]
fn opening_pushes_the_view_and_closing_pops_it() {
    let (mut app, _) = open_with(vec![notice(1, "a", "read")], 0);
    assert_eq!(app.view(), View::Notifications);
    dispatch(&mut app, NotificationAction::Close);
    assert_ne!(app.view(), View::Notifications);
    assert!(app.notifications.panel.is_none());
}

#[test]
fn only_the_focused_unread_notice_is_marked_read() {
    let (mut app, effects) =
        open_with(vec![notice(2, "top", "delivered"), notice(1, "below", "pending")], 2);
    assert_eq!(read_id(&effects), Some(uuid::Uuid::from_u128(2).to_string()));
    assert_eq!(app.notifications.items[1].state, "pending", "not on focus yet");

    let next = dispatch(&mut app, NotificationAction::SelectNext);
    assert_eq!(read_id(&next), Some(uuid::Uuid::from_u128(1).to_string()));
    assert!(dispatch(&mut app, NotificationAction::SelectPrev).is_empty(), "already read");
}

#[test]
fn a_closed_panel_never_marks_anything_read() {
    let mut app = App::new();
    assert!(loaded(&mut app, vec![notice(1, "a", "delivered")], 1).is_empty());
    assert_eq!(app.notifications.items[0].state, "delivered");
}

#[test]
fn a_digit_answers_with_that_action_from_the_tui() {
    let (mut app, _) = open_with(vec![notice(1, "a", "read")], 0);
    let effects = dispatch(&mut app, NotificationAction::Act(1));
    assert!(matches!(
        effects.as_slice(),
        [Effect::Notifications(NotificationEffect::Act { action, .. })] if action == "reject"
    ));
    assert!(app.notifications.panel.as_ref().is_some_and(|p| p.busy));
    assert!(dispatch(&mut app, NotificationAction::Act(0)).is_empty(), "one answer in flight");
}

#[test]
fn a_digit_past_the_actions_does_nothing() {
    let (mut app, _) = open_with(vec![notice(1, "a", "read")], 0);
    assert!(dispatch(&mut app, NotificationAction::Act(5)).is_empty());
    let mut answered = notice(1, "a", "acted");
    answered.decision = Some("approve".to_owned());
    dispatch(&mut app, NotificationAction::Upsert(Box::new(answered)));
    assert!(dispatch(&mut app, NotificationAction::Act(0)).is_empty(), "already answered");
}

#[test]
fn losing_the_race_is_news_not_an_error() {
    let (mut app, _) = open_with(vec![notice(1, "a", "read")], 0);
    dispatch(&mut app, NotificationAction::Act(0));
    let mut won = notice(1, "a", "acted");
    won.decision = Some("reject".to_owned());
    won.acted_channel = Some("ntfy".to_owned());
    let t = NotificationTransition { notification: won, applied: false };
    dispatch(&mut app, NotificationAction::Acted(Box::new(t)));

    let toast = app.toasts.latest().expect("a toast");
    assert_eq!(toast.level, crate::app::toast::Level::Info);
    assert!(toast.text.contains("reject via ntfy"), "{}", toast.text);
    assert_eq!(app.notifications.items[0].state, "acted");
    assert!(app.notifications.panel.as_ref().is_some_and(|p| !p.busy));
}

#[test]
fn dismiss_and_read_all_go_to_the_server() {
    let (mut app, _) = open_with(vec![notice(1, "a", "read")], 0);
    assert!(matches!(
        dispatch(&mut app, NotificationAction::Dismiss).as_slice(),
        [Effect::Notifications(NotificationEffect::Dismiss { .. })]
    ));
    dispatch(&mut app, NotificationAction::Dismissed(Box::new(notice(1, "a", "resolved"))));
    assert!(app.notifications.items.is_empty());
    assert!(matches!(
        dispatch(&mut app, NotificationAction::ReadAll).as_slice(),
        [Effect::Notifications(NotificationEffect::ReadAll)]
    ));
    assert!(matches!(
        dispatch(&mut app, NotificationAction::ReadAllDone(0)).as_slice(),
        [Effect::Notifications(NotificationEffect::Fetch)]
    ));
}

#[test]
fn a_refused_dismiss_says_why() {
    let (mut app, _) = open_with(vec![notice(1, "a", "read")], 0);
    dispatch(&mut app, NotificationAction::Dismiss);
    let refused = cctui_client::ClientError::Status {
        route: "post_notifications_by_id_dismiss",
        status: 409,
        body: String::new(),
    };
    let message = super::dismiss_refused(&refused).expect("a 409 is a refusal");
    dispatch(&mut app, NotificationAction::WriteFailed(message));
    assert!(app.toasts.latest().is_some_and(|t| t.text.contains("answer it instead")));
    assert_eq!(app.notifications.items.len(), 1);
}

#[test]
fn actions_read_the_record_and_vanish_once_answered() {
    let n = notice(1, "a", "delivered");
    let labels: Vec<String> = actions(&n).into_iter().map(|a| a.label).collect();
    assert_eq!(labels, ["Approve", "Reject"]);
    assert!(actions(&notice(1, "a", "acted")).is_empty());
}

#[test]
fn the_list_view_shows_the_focused_notice_and_its_actions() {
    let (mut app, _) =
        open_with(vec![notice(2, "Release 1.2?", "read"), notice(1, "Older", "pending")], 1);
    let screen = crate::testsupport::render_screen(&mut app);
    assert!(screen.contains("Notifications · 1 unread"), "{screen}");
    assert!(screen.contains("❯   approval     Release 1.2?"), "{screen}");
    assert!(screen.contains("● approval     Older"), "{screen}");
    assert!(screen.contains("ship it to prod"), "{screen}");
    assert!(screen.contains(" 1 Approve  2 Reject"), "{screen}");
}

#[test]
fn the_list_view_says_when_it_is_empty() {
    let (mut app, _) = open_with(Vec::new(), 0);
    let screen = crate::testsupport::render_screen(&mut app);
    assert!(screen.contains("no open notifications"), "{screen}");
}

#[test]
fn the_status_line_carries_the_unread_count_and_its_key() {
    let mut app = crate::testsupport::app_with_sessions();
    dispatch(&mut app, NotificationAction::Unread(3));
    let screen = crate::testsupport::render_screen_sized(&mut app, 200, 24);
    assert!(screen.contains("✉ 3 unread — B"), "{screen}");
}
