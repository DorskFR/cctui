use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::Response;
use axum::{Extension, Json};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{
    DismissQuery, ListQuery, NotificationActRequest, NotificationTransition, act, act_with_token,
    confirm_page, dismiss, list, read, read_all,
};
use crate::auth::{AuthContext, Scope};
use crate::notify::store::{self, NewNotification, NotificationRecord, Routing, emit};
use crate::notify::tokens;
use crate::routes::device_auth::PeerAddr;
use crate::state::AppState;

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(Scope::Read).collect(),
    }
}

async fn state(name: &str) -> Option<AppState> {
    let url = crate::routes::gateway::test_db_url(name)?;
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    Some(AppState::for_test(pool))
}

async fn user(state: &AppState) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(id)
        .bind(format!("notif-routes-{id}"))
        .bind(format!("notif-routes-key-{id}"))
        .execute(&state.pool)
        .await
        .unwrap();
    id
}

async fn drop_users(state: &AppState, ids: &[Uuid]) {
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(ids)
        .execute(&state.pool)
        .await
        .unwrap();
}

fn approve_reject() -> Value {
    json!([
        { "id": "approve", "label": "Approve" },
        { "id": "reject", "label": "Reject" },
        { "id": "deploy", "label": "Deploy", "requires_login": true },
    ])
}

async fn notice(state: &AppState, owner: Uuid, kind: &'static str, actions: Value) -> Uuid {
    let mut n = NewNotification::new(owner, kind, "Release 1.2?".into());
    n.actions = actions;
    emit(&state.pool, Routing::default(), n).await.unwrap().id
}

async fn row(state: &AppState, id: Uuid) -> NotificationRecord {
    store::get(&state.pool, id).await.unwrap().unwrap()
}

fn act_req(action: &str) -> NotificationActRequest {
    NotificationActRequest { action: action.into(), note: None, channel: None }
}

async fn act_as(
    state: &AppState,
    who: Uuid,
    id: Uuid,
    action: &str,
) -> Result<NotificationTransition, crate::error::AppError> {
    act(State(state.clone()), Extension(caller(who)), Path(id), Json(act_req(action)))
        .await
        .map(|Json(t)| t)
}

async fn listed(state: &AppState, who: Uuid, query: ListQuery) -> super::NotificationList {
    list(State(state.clone()), Extension(caller(who)), Query(query)).await.unwrap().0
}

fn html() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(header::ACCEPT, HeaderValue::from_static("text/html,*/*"));
    headers
}

async fn post_token(state: &AppState, token: &str, headers: HeaderMap) -> Response {
    act_with_token(State(state.clone()), PeerAddr(None), headers, Path(token.to_owned()))
        .await
        .unwrap_or_else(axum::response::IntoResponse::into_response)
}

async fn body(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn token_outcome(state: &AppState, token: &str) -> NotificationTransition {
    let response = post_token(state, token, HeaderMap::new()).await;
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_str(&body(response).await).unwrap()
}

async fn mint(state: &AppState, id: Uuid, action: &str, ttl: Duration) -> Option<String> {
    tokens::mint(&state.pool, id, None, action, "ntfy", Utc::now() + ttl).await.unwrap()
}

#[tokio::test]
async fn another_users_notification_is_404() {
    let Some(state) = state("notif_routes_owner_scope").await else { return };
    let (alice, bob) = (user(&state).await, user(&state).await);
    let id = notice(&state, alice, "approval", approve_reject()).await;

    let status = |r: Result<(), crate::error::AppError>| r.unwrap_err().status();
    let read_it = read(State(state.clone()), Extension(caller(bob)), Path(id)).await;
    assert_eq!(status(read_it.map(|_| ())), StatusCode::NOT_FOUND);
    let acted = act_as(&state, bob, id, "approve").await;
    assert_eq!(status(acted.map(|_| ())), StatusCode::NOT_FOUND);
    let dismissed = dismiss_as(&state, bob, id, false).await;
    assert_eq!(status(dismissed.map(|_| ())), StatusCode::NOT_FOUND);
    assert!(listed(&state, bob, ListQuery::default()).await.notifications.is_empty());
    assert_eq!(row(&state, id).await.state, "pending");

    let mine = listed(&state, alice, ListQuery::default()).await;
    assert_eq!(mine.notifications.iter().map(|n| n.id).collect::<Vec<_>>(), [id]);
    assert_eq!(mine.unread, 1);
    drop_users(&state, &[alice, bob]).await;
}

#[tokio::test]
async fn the_second_answer_is_not_applied_and_reads_the_first() {
    let Some(state) = state("notif_routes_act_once").await else { return };
    let alice = user(&state).await;
    let id = notice(&state, alice, "approval", approve_reject()).await;

    let first = act_as(&state, alice, id, "approve").await.unwrap();
    assert!(first.applied);
    assert_eq!(first.notification.decision.as_deref(), Some("approve"));
    assert_eq!(first.notification.acted_channel.as_deref(), Some("web"));
    assert_eq!(first.notification.acted_by, Some(alice));

    let second = act_as(&state, alice, id, "reject").await.unwrap();
    assert!(!second.applied);
    assert_eq!(second.notification.state, "acted");
    assert_eq!(second.notification.decision.as_deref(), Some("approve"));

    let unknown = act_as(&state, alice, id, "merge").await.unwrap_err();
    assert_eq!(unknown.status(), StatusCode::BAD_REQUEST);
    drop_users(&state, &[alice]).await;
}

async fn dismiss_as(
    state: &AppState,
    who: Uuid,
    id: Uuid,
    confirm: bool,
) -> Result<NotificationTransition, crate::error::AppError> {
    let query = Query(DismissQuery { confirm });
    dismiss(State(state.clone()), Extension(caller(who)), Path(id), query).await.map(|Json(t)| t)
}

async fn outcome_row(state: &AppState, session: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO session_outcomes (session_id, status, headline, source) \
         VALUES ($1, 'needs_input', 'Ship it?', 'tool') RETURNING id",
    )
    .bind(session)
    .fetch_one(&state.pool)
    .await
    .unwrap()
}

async fn session_approval(state: &AppState, owner: Uuid, session: &str) -> Uuid {
    let mut n = NewNotification::new(owner, "approval", "Ship it?".into());
    n.actions = approve_reject();
    n.blocking = true;
    n.session_id = Some(session.to_owned());
    n.subject_kind = Some("outcome".into());
    n.subject_id = Some(outcome_row(state, session).await.to_string());
    n.collapse_key = Some(format!("approval:{session}"));
    emit(&state.pool, Routing::default(), n).await.unwrap().id
}

async fn decision_events(state: &AppState, id: Uuid) -> Vec<Value> {
    sqlx::query_scalar("SELECT payload FROM inbox_events WHERE source_id = $1")
        .bind(id.to_string())
        .fetch_all(&state.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_live_approval_needs_the_confirm_path_and_then_expires() {
    let Some(state) = state("notif_routes_dismiss_live").await else { return };
    let (alice, sid) = crate::inbox::store_tests::seed_session(&state.pool, "dismiss-live").await;
    let approval = session_approval(&state, alice, &sid).await;

    let refused = dismiss_as(&state, alice, approval, false).await.unwrap_err();
    assert_eq!(refused.status(), StatusCode::CONFLICT);
    assert_eq!(row(&state, approval).await.state, "pending");
    assert!(decision_events(&state, approval).await.is_empty());

    let done = dismiss_as(&state, alice, approval, true).await.unwrap();
    assert!(done.applied);
    assert_eq!(done.notification.state, "expired");
    assert_eq!(done.notification.decision.as_deref(), Some("expired"));
    let events = decision_events(&state, approval).await;
    assert_eq!(events.len(), 1, "the run hears the dismissal");
    assert_eq!(events[0]["decision"], "expired");
    assert_eq!(events[0]["session_id"], sid.as_str());
}

#[tokio::test]
async fn a_stale_approval_dismisses_without_confirm() {
    let Some(state) = state("notif_routes_dismiss_stale").await else { return };
    let seed = crate::inbox::store_tests::seed_session;
    let (alice, ended) = seed(&state.pool, "dismiss-ended").await;
    let (_, archived) = seed(&state.pool, "dismiss-archived").await;
    let (_, moved) = seed(&state.pool, "dismiss-moved").await;
    let gone = [
        session_approval(&state, alice, &ended).await,
        session_approval(&state, alice, &archived).await,
        session_approval(&state, alice, &moved).await,
        notice(&state, alice, "approval", approve_reject()).await,
    ];
    for (sid, status) in [(&ended, "ended"), (&archived, "archived")] {
        sqlx::query("UPDATE sessions SET status = $2 WHERE id = $1")
            .bind(sid)
            .bind(status)
            .execute(&state.pool)
            .await
            .unwrap();
    }
    outcome_row(&state, &moved).await;

    for id in gone {
        let done = dismiss_as(&state, alice, id, false).await.unwrap();
        assert!(done.applied);
        assert_eq!(done.notification.state, "resolved");
        assert_eq!(done.notification.decision.as_deref(), Some("dismissed"));
        assert!(decision_events(&state, id).await.is_empty(), "nobody is waiting on it");
    }

    let info = notice(&state, alice, "outcome_result", json!([])).await;
    let done = dismiss_as(&state, alice, info, false).await.unwrap();
    assert_eq!(done.notification.decision.as_deref(), Some("dismissed"));
}

#[tokio::test]
async fn read_all_reads_every_notice_and_keeps_actions_answerable() {
    let Some(state) = state("notif_routes_read_all").await else { return };
    let alice = user(&state).await;
    let approval = notice(&state, alice, "approval", approve_reject()).await;
    let info = notice(&state, alice, "outcome_result", json!([])).await;

    let Json(bulk) = read_all(State(state.clone()), Extension(caller(alice))).await.unwrap();
    assert_eq!((bulk.read, bulk.unread), (2, 0));
    assert_eq!(row(&state, info).await.state, "read");
    let still_open = row(&state, approval).await;
    assert_eq!(still_open.state, "read");
    assert!(store::needs_answer(&still_open));
    assert_eq!(listed(&state, alice, ListQuery::default()).await.notifications.len(), 2);

    let answered = act_as(&state, alice, approval, "approve").await.unwrap();
    assert!(answered.applied);
    assert_eq!(answered.notification.decision.as_deref(), Some("approve"));
    assert!(!store::needs_answer(&answered.notification));
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn a_token_acts_once() {
    let Some(state) = state("notif_token_single_use").await else { return };
    let alice = user(&state).await;
    let id = notice(&state, alice, "approval", approve_reject()).await;
    let token = mint(&state, id, "approve", Duration::hours(1)).await.expect("token");
    let other = mint(&state, id, "reject", Duration::hours(1)).await.expect("token");

    let first = token_outcome(&state, &token).await;
    assert!(first.applied);
    assert_eq!(first.notification.decision.as_deref(), Some("approve"));
    assert_eq!(first.notification.acted_channel.as_deref(), Some("ntfy"));

    let again = token_outcome(&state, &token).await;
    assert!(!again.applied);
    assert_eq!(again.notification.decision.as_deref(), Some("approve"));

    let sibling = token_outcome(&state, &other).await;
    assert!(!sibling.applied, "closing the notice burns its other tokens");
    assert_eq!(sibling.notification.decision.as_deref(), Some("approve"));

    let page = body(post_token(&state, &token, html()).await).await;
    assert!(page.contains("Already answered from ntfy: approve."), "{page}");
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn an_expired_token_does_not_act() {
    let Some(state) = state("notif_token_expired").await else { return };
    let alice = user(&state).await;
    let id = notice(&state, alice, "approval", approve_reject()).await;
    let token = mint(&state, id, "approve", Duration::minutes(-1)).await.expect("token");

    let outcome = token_outcome(&state, &token).await;
    assert!(!outcome.applied);
    assert_eq!(outcome.notification.state, "pending");
    assert_eq!(row(&state, id).await.decision, None);

    let page = body(confirm_page(State(state.clone()), Path(token)).await).await;
    assert!(!page.contains("<form"), "{page}");
    assert!(page.contains("already used or has expired"), "{page}");
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn an_unknown_token_is_404() {
    let Some(state) = state("notif_token_unknown").await else { return };
    let junk = post_token(&state, "nope", HeaderMap::new()).await;
    assert_eq!(junk.status(), StatusCode::NOT_FOUND);
    let unminted = URL_SAFE_NO_PAD.encode([7u8; 32]);
    assert_eq!(post_token(&state, &unminted, html()).await.status(), StatusCode::NOT_FOUND);
    let page = confirm_page(State(state), Path(unminted)).await;
    assert_eq!(page.status(), StatusCode::NOT_FOUND);
}

async fn failed_dispatch(state: &AppState, owner: Uuid) -> (String, Uuid) {
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO dispatch_records (session_id, user_id, dispatcher_name, outcome) \
         VALUES ($1, $2, 'k8s', 'refused')",
    )
    .bind(&sid)
    .bind(owner)
    .execute(&state.pool)
    .await
    .unwrap();
    let kind = crate::notify::kind::DISPATCH_FAILED;
    let mut n = NewNotification::new(owner, kind, "x failed".into());
    n.subject_kind = Some(crate::notify::producers::DISPATCH_SUBJECT.into());
    n.subject_id = Some(sid.clone());
    n.collapse_key = Some(format!("dispatch_failed:{sid}"));
    (sid, emit(&state.pool, Routing::default(), n).await.unwrap().id)
}

async fn shown_failures(state: &AppState, who: Uuid) -> Vec<String> {
    use cctui_server_dispatch::execution_failures::list;
    use cctui_server_dispatch::store::dispatch_records::Listing;
    let listing = Listing { owner: Some(who), viewer: Some(who), include_dismissed: false };
    list(&state.pool, listing).await.unwrap().into_iter().map(|f| f.session_id).collect()
}

#[tokio::test]
async fn dismissing_a_dispatch_notice_dismisses_its_failure_record() {
    let Some(state) = state("notif_routes_dispatch_notice_dismiss").await else { return };
    let alice = user(&state).await;
    let (sid, id) = failed_dispatch(&state, alice).await;
    assert_eq!(shown_failures(&state, alice).await, [sid]);

    let done = dismiss_as(&state, alice, id, false).await.unwrap();
    assert!(done.applied);
    assert!(shown_failures(&state, alice).await.is_empty(), "the record goes with its notice");
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn dismissing_failure_records_resolves_their_notices() {
    use cctui_server_dispatch::routes::dispatch::{
        dismiss_all_failed_dispatches, dismiss_failed_dispatch,
    };
    let Some(state) = state("notif_routes_dispatch_record_dismiss").await else { return };
    let alice = user(&state).await;
    let (one, one_notice) = failed_dispatch(&state, alice).await;
    let (_two, two_notice) = failed_dispatch(&state, alice).await;
    let other = notice(&state, alice, "system_alert", json!([])).await;

    let status = dismiss_failed_dispatch(State(state.clone()), Extension(caller(alice)), Path(one))
        .await
        .unwrap();
    assert_eq!(status, StatusCode::NO_CONTENT);
    let closed = row(&state, one_notice).await;
    assert_eq!(
        (closed.state.as_str(), closed.resolved_reason.as_deref()),
        ("resolved", Some("subject_closed"))
    );
    assert_eq!(row(&state, two_notice).await.state, "pending", "only the dismissed record's");

    let Json(all) = dismiss_all_failed_dispatches(State(state.clone()), Extension(caller(alice)))
        .await
        .unwrap();
    assert_eq!(all.dismissed, 1);
    assert_eq!(row(&state, two_notice).await.state, "resolved");
    assert_eq!(row(&state, other).await.state, "pending", "other notices stay open");
    drop_users(&state, &[alice]).await;
}
