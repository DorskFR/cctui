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
    ListQuery, NotificationActRequest, NotificationTransition, act, act_with_token, confirm_page,
    dismiss, escape, list, page, read, read_all, validate_act,
};
use crate::auth::{AuthContext, Scope};
use crate::notify::store::{self, Edge, NewNotification, NotificationRecord, Routing, emit};
use crate::notify::tokens;
use crate::routes::device_auth::PeerAddr;
use crate::state::AppState;

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: [Scope::Read].into_iter().collect(),
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

#[test]
fn page_defaults_to_open_and_clamps_the_limit() {
    let q = |state: Option<&str>, limit| ListQuery {
        state: state.map(Into::into),
        limit,
        before: None,
    };
    let p = page(&q(None, None)).unwrap();
    assert!(p.open_only);
    assert_eq!(p.limit, 50);
    assert!(!page(&q(Some("all"), Some(0))).unwrap().open_only);
    assert_eq!(page(&q(Some("all"), Some(0))).unwrap().limit, 1);
    assert_eq!(page(&q(None, Some(10_000))).unwrap().limit, 200);
    assert!(page(&q(Some("closed"), None)).unwrap_err().contains("open or all"));
}

#[test]
fn act_validation_names_what_the_notice_offers() {
    let n: NotificationRecord = serde_json::from_value(json!({
        "id": Uuid::nil(), "user_id": Uuid::nil(), "kind": "approval", "severity": "info",
        "title": "t", "actions": approve_reject(), "state": "read", "repeat_count": 1,
        "created_at": "2026-11-16T10:00:00Z", "updated_at": "2026-11-16T10:00:00Z",
    }))
    .unwrap();
    let ok = NotificationActRequest {
        action: " approve ".into(),
        note: Some("  ".into()),
        channel: Some("tui".into()),
    };
    assert_eq!(validate_act(&n, &ok).unwrap(), ("approve", None, "tui"));
    let err = validate_act(&n, &act_req("merge")).unwrap_err();
    assert!(err.contains("approve, reject, deploy"), "{err}");
    let bad_channel = NotificationActRequest { channel: Some("ntfy".into()), ..act_req("approve") };
    assert!(validate_act(&n, &bad_channel).unwrap_err().contains("web or tui"));
    let long = NotificationActRequest { note: Some("x".repeat(1001)), ..act_req("approve") };
    assert!(validate_act(&n, &long).unwrap_err().contains("1000"));
}

#[test]
fn escape_neutralises_markup() {
    assert_eq!(escape(r#"<b a="1">&'"#), "&lt;b a=&quot;1&quot;&gt;&amp;&#39;");
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
    let dismissed = dismiss(State(state.clone()), Extension(caller(bob)), Path(id)).await;
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

#[tokio::test]
async fn a_blocking_approval_cannot_be_dismissed() {
    let Some(state) = state("notif_routes_dismiss").await else { return };
    let alice = user(&state).await;
    let approval = notice(&state, alice, "approval", approve_reject()).await;
    let info = notice(&state, alice, "outcome_result", json!([])).await;

    let refused = dismiss(State(state.clone()), Extension(caller(alice)), Path(approval)).await;
    assert_eq!(refused.map(|_| ()).unwrap_err().status(), StatusCode::CONFLICT);
    assert_eq!(row(&state, approval).await.state, "pending");

    let Json(done) =
        dismiss(State(state.clone()), Extension(caller(alice)), Path(info)).await.unwrap();
    assert!(done.applied);
    assert_eq!(done.notification.state, "resolved");
    assert_eq!(done.notification.decision.as_deref(), Some("dismissed"));
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn read_all_leaves_notices_with_actions_unread() {
    let Some(state) = state("notif_routes_read_all").await else { return };
    let alice = user(&state).await;
    let approval = notice(&state, alice, "approval", approve_reject()).await;
    let info = notice(&state, alice, "outcome_result", json!([])).await;

    let Json(bulk) = read_all(State(state.clone()), Extension(caller(alice))).await.unwrap();
    assert_eq!((bulk.read, bulk.unread), (1, 1));
    assert_eq!(row(&state, info).await.state, "read");
    assert_eq!(row(&state, approval).await.state, "pending");

    let Json(one) =
        read(State(state.clone()), Extension(caller(alice)), Path(approval)).await.unwrap();
    assert!(one.applied);
    assert_eq!(listed(&state, alice, ListQuery::default()).await.unread, 0);
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn the_list_pages_newest_first_and_hides_closed_notices() {
    let Some(state) = state("notif_routes_list").await else { return };
    let alice = user(&state).await;
    let mut ids = Vec::new();
    for _ in 0..3 {
        ids.push(notice(&state, alice, "outcome_result", json!([])).await);
    }
    store::transition(&state.pool, ids[0], Edge::Dismiss).await.unwrap();

    let open = listed(&state, alice, ListQuery::default()).await;
    assert_eq!(open.notifications.iter().map(|n| n.id).collect::<Vec<_>>(), [ids[2], ids[1]]);
    assert!(!open.has_more);

    let all = |before| ListQuery { state: Some("all".into()), limit: Some(2), before };
    let first = listed(&state, alice, all(None)).await;
    assert_eq!(first.notifications.len(), 2);
    assert!(first.has_more);
    let next = listed(&state, alice, all(Some(first.notifications[1].created_at))).await;
    assert_eq!(next.notifications.iter().map(|n| n.id).collect::<Vec<_>>(), [ids[0]]);
    assert!(!next.has_more);
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
async fn the_confirm_page_never_changes_state() {
    let Some(state) = state("notif_confirm_page_get").await else { return };
    let alice = user(&state).await;
    let id = notice(&state, alice, "approval", approve_reject()).await;
    let token = mint(&state, id, "approve", Duration::hours(1)).await.expect("token");
    let before = row(&state, id).await;

    for _ in 0..2 {
        let response = confirm_page(State(state.clone()), Path(token.clone())).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.headers()[header::REFERRER_POLICY], "no-referrer");
        let page = body(response).await;
        assert!(page.contains(&format!("action=\"/api/v1/notifications/actions/{token}\"")));
        assert!(page.contains(">Approve</button>"), "{page}");
    }
    assert_eq!(row(&state, id).await, before);
    let hash = tokens::hash(&token).unwrap();
    assert!(tokens::peek(&state.pool, &hash).await.unwrap().unwrap().usable);
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn no_token_for_login_only_unknown_or_closed_actions() {
    let Some(state) = state("notif_token_mint_refusals").await else { return };
    let alice = user(&state).await;
    let id = notice(&state, alice, "approval", approve_reject()).await;
    assert_eq!(mint(&state, id, "deploy", Duration::hours(1)).await, None);
    assert_eq!(mint(&state, id, "merge", Duration::hours(1)).await, None);
    assert_eq!(mint(&state, Uuid::new_v4(), "approve", Duration::hours(1)).await, None);
    store::transition(&state.pool, id, Edge::Expire).await.unwrap();
    assert_eq!(mint(&state, id, "approve", Duration::hours(1)).await, None);
    drop_users(&state, &[alice]).await;
}

#[tokio::test]
async fn a_token_never_outlives_a_day() {
    let Some(state) = state("notif_token_lifetime_cap").await else { return };
    let alice = user(&state).await;
    let id = notice(&state, alice, "approval", approve_reject()).await;
    let token = mint(&state, id, "approve", Duration::days(30)).await.expect("token");
    let capped: bool = sqlx::query_scalar(
        "SELECT expires_at <= now() + interval '24 hours' FROM notification_action_tokens \
         WHERE token_hash = $1",
    )
    .bind(tokens::hash(&token).unwrap())
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert!(capped);
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
