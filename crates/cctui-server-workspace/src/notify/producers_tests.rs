use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::{Extension, Json};
use cctui_proto::api::outcome::{OutcomeSource, OutcomeStatus, SessionOutcome};
use cctui_proto::api::{
    AddUserActionRequest, UserActionKind, UserActionResolver, UserActionStatus,
};
use chrono::Utc;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::{AddedAction, outcome_declared, user_action_added};
use crate::auth::{AuthContext, Scope};
use crate::inbox::store_tests::seed_session;
use crate::notify::store::{self, Edge, NotificationRecord, Routing};
use crate::routes::device_auth::PeerAddr;
use crate::routes::user_actions::{self, Ticked};
use crate::state::AppState;

async fn state(name: &str) -> Option<AppState> {
    let url = crate::routes::gateway::test_db_url(name)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(6)
        .connect(&url)
        .await
        .expect("connect test db");
    Some(AppState::for_test(pool))
}

fn request(title: &str, blocking: bool) -> AddUserActionRequest {
    AddUserActionRequest {
        title: title.to_owned(),
        detail: Some("plug it in".to_owned()),
        kind: UserActionKind::Action,
        blocking,
    }
}

async fn add(state: &AppState, sid: &str, title: &str, blocking: bool) -> Uuid {
    let result = user_actions::add(state, sid, &request(title, blocking)).await.unwrap();
    assert!(result.error.is_none(), "{:?}", result.error);
    result.added.unwrap()
}

async fn notices_for(pool: &PgPool, action_id: Uuid) -> Vec<NotificationRecord> {
    sqlx::query_as(
        "SELECT * FROM notifications WHERE subject_kind = 'user_action' AND subject_id = $1",
    )
    .bind(action_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn notice_for(pool: &PgPool, action_id: Uuid) -> NotificationRecord {
    let mut all = notices_for(pool, action_id).await;
    assert_eq!(all.len(), 1, "one notification per user action");
    all.remove(0)
}

async fn action_row(pool: &PgPool, id: Uuid) -> (String, Option<String>, Option<String>) {
    sqlx::query_as("SELECT status, resolved_by, note FROM session_user_actions WHERE id = $1")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

struct DecisionEvent {
    source: String,
    lane: String,
    parent: Option<String>,
    payload: Value,
}

async fn decision_events(pool: &PgPool, notification_id: Uuid) -> Vec<DecisionEvent> {
    let rows: Vec<(String, String, Option<String>, Value)> = sqlx::query_as(
        "SELECT e.source, l.key, e.parent_session_id, e.payload FROM inbox_events e \
         JOIN inbox_lanes l ON l.id = e.lane_id WHERE e.source_id = $1",
    )
    .bind(notification_id.to_string())
    .fetch_all(pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|(source, lane, parent, payload)| DecisionEvent { source, lane, parent, payload })
        .collect()
}

fn needs_input(sid: &str) -> SessionOutcome {
    SessionOutcome {
        id: Uuid::new_v4(),
        session_id: sid.to_owned(),
        dispatch_id: None,
        turn_seq: None,
        status: OutcomeStatus::NeedsInput,
        headline: "Ship release 1.2?".to_owned(),
        body: Some("All checks green.".to_owned()),
        evidence: Vec::new(),
        source: OutcomeSource::Tool,
        created_at: Utc::now(),
    }
}

async fn approval_for(pool: &PgPool, sid: &str) -> NotificationRecord {
    outcome_declared(pool, Routing::default(), &needs_input(sid)).await.unwrap().unwrap()
}

fn approve<'a>(note: Option<&'a str>, by: Uuid) -> Edge<'a> {
    Edge::Act { action: "approve", note, channel: "web", by: Some(by) }
}

#[tokio::test]
async fn adding_a_blocking_user_action_creates_one_notification() {
    let Some(state) = state("producer_user_action_add").await else { return };
    let (u, sid) = seed_session(&state.pool, "producer-ua-add").await;
    let id = add(&state, &sid, "Plug the yubikey", true).await;
    assert_eq!(add(&state, &sid, "Plug the yubikey", true).await, id, "same open item");

    let n = notice_for(&state.pool, id).await;
    assert_eq!((n.kind.as_str(), n.severity.as_str()), ("user_action", "high"));
    assert_eq!((n.user_id, n.session_id.as_deref()), (u, Some(sid.as_str())));
    assert_eq!(n.collapse_key, Some(format!("user_action:{id}")));
    assert_eq!(n.body.as_deref(), Some("plug it in"));
    assert_eq!(store::action_ids(&n), ["done", "dropped"]);
    assert!(store::awaits_decision(&state.pool, &n).await.unwrap());

    let plain = add(&state, &sid, "Read the changelog", false).await;
    let n = notice_for(&state.pool, plain).await;
    assert!(!store::awaits_decision(&state.pool, &n).await.unwrap());
}

#[tokio::test]
async fn a_blocking_user_action_schedules_a_delayed_push_and_a_plain_one_never_pushes() {
    let Some(state) = state("producer_user_action_push").await else { return };
    let (_, sid) = seed_session(&state.pool, "producer-ua-push").await;
    let routing = Routing { ntfy: true };
    let mut pushes = Vec::new();
    for blocking in [true, false] {
        let added = AddedAction {
            session_id: &sid,
            id: Uuid::new_v4(),
            title: "Plug the yubikey",
            detail: None,
            blocking,
        };
        let n = user_action_added(&state.pool, routing, &added).await.unwrap().unwrap();
        let rows: Vec<(String, i32, bool)> = sqlx::query_as(
            "SELECT channel, stage, next_attempt_at > now() + interval '9 minutes' \
             FROM notification_deliveries WHERE notification_id = $1 AND channel = 'ntfy'",
        )
        .bind(n.id)
        .fetch_all(&state.pool)
        .await
        .unwrap();
        pushes.push(rows);
    }
    assert_eq!(pushes[0], [("ntfy".to_owned(), 1, true)], "escalation-ready, not sent now");
    assert!(pushes[1].is_empty(), "a non-blocking action never pushes");
}

#[tokio::test]
async fn ticking_a_user_action_resolves_its_notification() {
    let Some(state) = state("producer_user_action_agent_tick").await else { return };
    let (_, sid) = seed_session(&state.pool, "producer-ua-agent").await;
    let id = add(&state, &sid, "Plug the yubikey", true).await;
    let by = UserActionResolver::Agent;
    let ticked =
        user_actions::tick(&state, &sid, id, UserActionStatus::Done, None, by).await.unwrap();
    assert!(matches!(ticked, Ticked::Applied(_)));

    let n = notice_for(&state.pool, id).await;
    assert_eq!(n.state, "resolved");
    assert_eq!(n.resolved_reason.as_deref(), Some("subject_closed"));
    assert!(decision_events(&state.pool, n.id).await.is_empty(), "the agent already knows");
}

#[tokio::test]
async fn a_user_tick_on_a_blocking_action_answers_its_notification_in_the_same_transaction() {
    let Some(state) = state("producer_user_action_user_tick").await else { return };
    let (_, sid) = seed_session(&state.pool, "producer-ua-user").await;
    let id = add(&state, &sid, "Plug the yubikey", true).await;
    user_actions::tick(
        &state,
        &sid,
        id,
        UserActionStatus::Done,
        Some("plugged"),
        UserActionResolver::User,
    )
    .await
    .unwrap();

    let n = notice_for(&state.pool, id).await;
    assert_eq!((n.state.as_str(), n.decision.as_deref()), ("acted", Some("done")));
    let events = decision_events(&state.pool, n.id).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["subject"], json!({ "kind": "user_action", "id": id }));
    assert_eq!(events[0].payload["note"], "plugged");
}

#[tokio::test]
async fn acting_on_the_notification_ticks_the_user_action() {
    let Some(state) = state("producer_act_ticks_user_action").await else { return };
    let (u, sid) = seed_session(&state.pool, "producer-act-tick").await;
    let id = add(&state, &sid, "Plug the yubikey", true).await;
    let n = notice_for(&state.pool, id).await;

    let edge = Edge::Act { action: "dropped", note: Some("no key"), channel: "web", by: Some(u) };
    let acted = store::act(&state.pool, n.id, edge).await.unwrap().unwrap();
    assert!(acted.transition.applied);
    assert_eq!(acted.ticked_session.as_deref(), Some(sid.as_str()));
    assert!(acted.decision_lane.is_some());
    assert_eq!(
        action_row(&state.pool, id).await,
        ("dropped".to_owned(), Some("user".to_owned()), Some("no key".to_owned()))
    );

    let by = UserActionResolver::Agent;
    let late =
        user_actions::tick(&state, &sid, id, UserActionStatus::Done, None, by).await.unwrap();
    assert!(matches!(late, Ticked::AlreadyResolved(..)));
    assert_eq!(notice_for(&state.pool, id).await.state, "acted");
    assert_eq!(decision_events(&state.pool, n.id).await.len(), 1);
}

#[tokio::test]
async fn a_needs_input_outcome_creates_an_approval_notification() {
    let Some(state) = state("producer_outcome_approval").await else { return };
    let (u, sid) = seed_session(&state.pool, "producer-outcome").await;
    let outcome = needs_input(&sid);
    let n = outcome_declared(&state.pool, Routing::default(), &outcome).await.unwrap().unwrap();
    assert_eq!((n.kind.as_str(), n.user_id), ("approval", u));
    assert_eq!(n.collapse_key, Some(format!("approval:{sid}")));
    let subject = outcome.id.to_string();
    assert_eq!(n.subject_kind.as_deref(), Some("outcome"));
    assert_eq!(n.subject_id.as_deref(), Some(subject.as_str()));
    assert_eq!(n.title, "Ship release 1.2?");
    assert_eq!(store::action_ids(&n), ["approve", "reject"]);
    assert!(
        n.actions.as_array().unwrap().iter().all(|a| a["requires_login"] == true),
        "an outcome approval is never answered from an unauthenticated link"
    );

    let mut result = needs_input(&sid);
    result.status = OutcomeStatus::Result;
    let closed = outcome_declared(&state.pool, Routing::default(), &result).await.unwrap().unwrap();
    assert_eq!((closed.id, closed.state.as_str()), (n.id, "resolved"));
    assert_eq!(closed.resolved_reason.as_deref(), Some("superseded"));
    assert!(decision_events(&state.pool, n.id).await.is_empty());
}

#[tokio::test]
async fn approving_writes_exactly_one_inbox_event_with_the_contract_payload() {
    let Some(state) = state("producer_approve_event").await else { return };
    let (u, sid) = seed_session(&state.pool, "producer-approve").await;
    let n = approval_for(&state.pool, &sid).await;

    let first = store::act(&state.pool, n.id, approve(Some("go"), u)).await.unwrap().unwrap();
    assert!(first.transition.applied);
    let second = store::act(&state.pool, n.id, approve(None, u)).await.unwrap().unwrap();
    assert!(!second.transition.applied);
    assert!(second.decision_lane.is_none());

    let events = decision_events(&state.pool, n.id).await;
    assert_eq!(events.len(), 1);
    let e = &events[0];
    assert_eq!(e.source, "approval");
    assert_eq!(e.lane, format!("session:{sid}"));
    assert_eq!(e.parent.as_deref(), Some(sid.as_str()));
    let p = &e.payload;
    assert_eq!(p["kind"], "notification.decision");
    assert_eq!(p["notification_id"], n.id.to_string());
    assert_eq!(p["session_id"], sid.as_str());
    assert_eq!(p["subject"]["kind"], "outcome");
    assert_eq!((p["decision"].as_str(), p["note"].as_str()), (Some("approve"), Some("go")));
    assert_eq!(p["actor"], json!({ "channel": "web", "user_id": u }));
    assert!(p["decided_at"].is_string());
    assert_eq!(p["idempotency_key"], n.id.to_string());
}

#[tokio::test]
async fn two_concurrent_acts_write_one_inbox_event() {
    let Some(state) = state("producer_concurrent_acts").await else { return };
    let (u, sid) = seed_session(&state.pool, "producer-race").await;
    let n = approval_for(&state.pool, &sid).await;
    let reject = Edge::Act { action: "reject", note: None, channel: "tui", by: Some(u) };
    let (a, b) = tokio::join!(
        store::act(&state.pool, n.id, approve(None, u)),
        store::act(&state.pool, n.id, reject),
    );
    let applied = [a.unwrap().unwrap(), b.unwrap().unwrap()]
        .iter()
        .filter(|acted| acted.transition.applied)
        .count();
    assert_eq!(applied, 1);
    assert_eq!(decision_events(&state.pool, n.id).await.len(), 1);
}

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(Scope::Read).collect(),
    }
}

#[tokio::test]
async fn the_bearer_and_token_act_routes_write_the_decision_event() {
    use crate::routes::notifications::{NotificationActRequest, act, act_with_token};
    let Some(state) = state("producer_act_routes").await else { return };
    let (u, sid) = seed_session(&state.pool, "producer-routes").await;

    let n = approval_for(&state.pool, &sid).await;
    let req = NotificationActRequest { action: "approve".into(), note: None, channel: None };
    let Json(done) =
        act(State(state.clone()), Extension(caller(u)), Path(n.id), Json(req)).await.unwrap();
    assert!(done.applied);
    assert_eq!(decision_events(&state.pool, n.id).await.len(), 1);

    let id = add(&state, &sid, "Plug the yubikey", true).await;
    let notice = notice_for(&state.pool, id).await;
    let expires = Utc::now() + chrono::Duration::hours(1);
    let token = crate::notify::tokens::mint(&state.pool, notice.id, None, "done", "ntfy", expires)
        .await
        .unwrap()
        .unwrap();
    let response =
        act_with_token(State(state.clone()), PeerAddr(None), HeaderMap::new(), Path(token))
            .await
            .unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    assert_eq!(action_row(&state.pool, id).await.0, "done", "the token act ticks the item");
    let events = decision_events(&state.pool, notice.id).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].payload["actor"]["channel"], "ntfy");
}
