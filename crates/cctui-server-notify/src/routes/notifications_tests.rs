use axum::Extension;
use axum::extract::{Path, Query, State};
use axum::http::{StatusCode, header};
use axum::response::Response;
use chrono::{Duration, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ListQuery, NotificationActRequest, confirm_page, escape, list, page, validate_act};
use crate::notify::store::{self, Edge, NewNotification, NotificationRecord, Routing, emit};
use crate::notify::tokens;
use cctui_server_kernel::auth::{AuthContext, Scope};
use cctui_server_kernel::test_support::TestHost;

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(Scope::Read).collect(),
    }
}

async fn state(name: &str) -> Option<TestHost> {
    let url = cctui_server_kernel::test_support::db_url(name)?;
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    Some(TestHost::new(pool))
}

async fn user(state: &TestHost) -> Uuid {
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

async fn drop_users(state: &TestHost, ids: &[Uuid]) {
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

async fn notice(state: &TestHost, owner: Uuid, kind: &'static str, actions: Value) -> Uuid {
    let mut n = NewNotification::new(owner, kind, "Release 1.2?".into());
    n.actions = actions;
    emit(&state.pool, Routing::default(), n).await.unwrap().id
}

async fn row(state: &TestHost, id: Uuid) -> NotificationRecord {
    store::get(&state.pool, id).await.unwrap().unwrap()
}

fn act_req(action: &str) -> NotificationActRequest {
    NotificationActRequest { action: action.into(), note: None, channel: None }
}

async fn listed(state: &TestHost, who: Uuid, query: ListQuery) -> super::NotificationList {
    list(State(state.clone()), Extension(caller(who)), Query(query)).await.unwrap().0
}

async fn body(response: Response) -> String {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

async fn mint(state: &TestHost, id: Uuid, action: &str, ttl: Duration) -> Option<String> {
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
