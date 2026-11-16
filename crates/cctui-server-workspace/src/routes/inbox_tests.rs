use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{CreateEvent, CreateEventResponse, ListQuery, cancel, create, get_one, list, validate};
use crate::auth::{AuthContext, Scope};
use crate::inbox::Source;
use crate::inbox::store_tests::seed_session;
use crate::state::AppState;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-16T10:00:00Z").unwrap().with_timezone(&Utc)
}

fn req(value: serde_json::Value) -> CreateEvent {
    serde_json::from_value(value).expect("valid request shape")
}

fn n8n(session_id: &str, source_id: &str) -> CreateEvent {
    req(json!({
        "session_id": session_id,
        "source": "n8n",
        "source_id": source_id,
        "body": "ticket updated",
    }))
}

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: [Scope::Read, Scope::Dispatch].into_iter().collect(),
    }
}

async fn state(name: &str) -> Option<AppState> {
    let url = crate::routes::gateway::test_db_url(name)?;
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    Some(AppState::for_test(pool))
}

async fn post(
    state: &AppState,
    user_id: Uuid,
    body: CreateEvent,
) -> Result<(StatusCode, Json<CreateEventResponse>), crate::error::AppError> {
    create(State(state.clone()), Extension(caller(user_id)), Json(body)).await
}

fn rejection(value: serde_json::Value) -> String {
    validate(req(value), now()).expect_err("rejected")
}

#[test]
fn a_session_event_validates() {
    let valid = validate(
        req(json!({
            "session_id": " s1 ",
            "source": "webhook",
            "source_id": "gh-delivery-1",
            "body": "push",
            "payload": {"ref": "main"},
            "not_before": "2026-11-16T11:00:00Z",
            "expires_at": "2026-11-16T13:00:00+01:00",
        })),
        now(),
    )
    .unwrap();
    assert_eq!(valid.session_id, "s1");
    assert_eq!(valid.source, Source::Webhook);
    assert_eq!(valid.source_id.as_deref(), Some("gh-delivery-1"));
    assert_eq!(valid.payload, Some(json!({"ref": "main"})));
    assert_eq!(valid.not_before, Some(now() + chrono::Duration::hours(1)));
    assert_eq!(valid.expires_at, Some(now() + chrono::Duration::hours(2)));
}

#[test]
fn external_sources_need_a_source_id_and_internal_ones_do_not() {
    for source in ["n8n", "webhook"] {
        let err = rejection(json!({"session_id": "s1", "source": source, "body": "x"}));
        assert!(err.contains("source_id is required"), "{source}: {err}");
        let blank = json!({"session_id": "s1", "source": source, "source_id": "  ", "body": "x"});
        let err = rejection(blank);
        assert!(err.contains("source_id is required"), "{source}: blank is missing");
    }
    let human = validate(req(json!({"session_id": "s1", "source": "human", "body": "x"})), now());
    assert_eq!(human.unwrap().source_id, None);
}

#[test]
fn only_session_targets_are_accepted() {
    let err = rejection(json!({"preset": "p1", "source": "n8n", "source_id": "a", "body": "x"}));
    assert!(err.contains("preset targets"), "{err}");
    let err = rejection(json!({"source": "n8n", "source_id": "a", "body": "x"}));
    assert!(err.contains("session_id is required"), "{err}");
    let err = rejection(
        json!({"session_id": "s1", "topic": "t", "source": "n8n", "source_id": "a", "body": "x"}),
    );
    assert!(err.contains("topic"), "{err}");
}

#[test]
fn malformed_events_are_refused() {
    let base = |extra: &serde_json::Value| {
        let mut v = json!({"session_id": "s1", "source": "timer", "body": "wake"});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        v
    };
    for (extra, needle) in [
        (json!({"source": "github"}), "unknown source"),
        (json!({"body": "  "}), "body must not be empty"),
        (json!({"body": "x".repeat(64 * 1024 + 1)}), "body must be at most"),
        (json!({"source_id": "x".repeat(257)}), "source_id must be at most"),
        (json!({"payload": [1, 2]}), "payload must be a JSON object"),
        (json!({"payload": {"blob": "x".repeat(9000)}}), "payload must be at most"),
        (json!({"payload": {"env": {"A": "b"}}}), "drop 'env'"),
        (json!({"payload": {"nested": [{"GitHub_Token": "t"}]}}), "drop 'GitHub_Token'"),
        (json!({"not_before": "tomorrow"}), "not_before must be an RFC3339"),
        (json!({"not_before": "2026-12-16T10:00:01Z"}), "at most 30 days"),
        (json!({"expires_at": "2026-11-16T09:00:00Z"}), "expires_at must be after"),
        (
            json!({"not_before": "2026-11-17T10:00:00Z", "expires_at": "2026-11-17T09:00:00Z"}),
            "expires_at must be after",
        ),
    ] {
        let err = rejection(base(&extra));
        assert!(err.contains(needle), "{extra}: {err}");
    }
    let smuggled = json!({"session_id": "s1", "source": "n8n", "body": "x", "reply_url": "u"});
    let unknown: Result<CreateEvent, _> = serde_json::from_value(smuggled);
    assert!(unknown.is_err(), "an unknown field is refused, not silently stored");
}

#[tokio::test]
async fn a_repeated_source_id_is_accepted_then_deduplicated() {
    let name = "inbox_route_dedup";
    let Some(state) = state(name).await else { return };
    let (uid, sid) = seed_session(&state.pool, name).await;

    let (code, Json(first)) = post(&state, uid, n8n(&sid, "d-1")).await.unwrap();
    assert_eq!((code, first.status.as_str()), (StatusCode::ACCEPTED, "accepted"));
    let (code, Json(again)) = post(&state, uid, n8n(&sid, "d-1")).await.unwrap();
    assert_eq!(
        (code, again.status.as_str(), again.id),
        (StatusCode::ACCEPTED, "deduplicated", first.id)
    );

    let missing = req(json!({"session_id": sid, "source": "n8n", "body": "x"}));
    let err = post(&state, uid, missing).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);

    let Json(event) = get_one(State(state), Extension(caller(uid)), Path(first.id)).await.unwrap();
    assert_eq!((event.status.as_str(), event.lane_key), ("pending", format!("session:{sid}")));
}

#[tokio::test]
async fn another_users_events_and_sessions_are_out_of_reach() {
    let name = "inbox_route_owner";
    let Some(state) = state(name).await else { return };
    let (owner, sid) = seed_session(&state.pool, name).await;
    let (stranger, _) = seed_session(&state.pool, name).await;
    let (_, Json(made)) = post(&state, owner, n8n(&sid, "o-1")).await.unwrap();

    let err = get_one(State(state.clone()), Extension(caller(stranger)), Path(made.id))
        .await
        .unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    let err =
        cancel(State(state.clone()), Extension(caller(stranger)), Path(made.id)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    let Json(seen) = list(
        State(state.clone()),
        Extension(caller(stranger)),
        Query(ListQuery { lane: Some(format!("session:{sid}")), status: None }),
    )
    .await
    .unwrap();
    assert!(seen.is_empty());

    let err = post(&state, stranger, n8n(&sid, "s-1")).await.unwrap_err();
    assert!(
        matches!(err.status(), StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "posting into another user's session: {}",
        err.status()
    );

    let Json(mine) = list(
        State(state.clone()),
        Extension(caller(owner)),
        Query(ListQuery { lane: None, status: Some("pending".into()) }),
    )
    .await
    .unwrap();
    assert_eq!(mine.iter().map(|e| e.id).collect::<Vec<_>>(), [made.id]);
    let err = list(
        State(state),
        Extension(caller(owner)),
        Query(ListQuery { lane: None, status: Some("sent".into()) }),
    )
    .await
    .unwrap_err();
    assert_eq!(err.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_delivered_event_cannot_be_cancelled() {
    let name = "inbox_route_cancel";
    let Some(state) = state(name).await else { return };
    let (uid, sid) = seed_session(&state.pool, name).await;
    let (_, Json(pending)) = post(&state, uid, n8n(&sid, "c-1")).await.unwrap();
    let (_, Json(delivered)) = post(&state, uid, n8n(&sid, "c-2")).await.unwrap();
    sqlx::query("UPDATE inbox_events SET status = 'delivered', delivered_at = now() WHERE id = $1")
        .bind(delivered.id)
        .execute(&state.pool)
        .await
        .unwrap();

    let err =
        cancel(State(state.clone()), Extension(caller(uid)), Path(delivered.id)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::CONFLICT);
    let code =
        cancel(State(state.clone()), Extension(caller(uid)), Path(pending.id)).await.unwrap();
    assert_eq!(code, StatusCode::NO_CONTENT);

    let read_only = AuthContext { scopes: std::iter::once(Scope::Read).collect(), ..caller(uid) };
    let err = cancel(State(state), Extension(read_only), Path(pending.id)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
}
