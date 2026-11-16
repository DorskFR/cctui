use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde_json::json;
use uuid::Uuid;

use super::{
    CreateEvent, CreateEventResponse, DEFAULT_TOPIC, ListQuery, Target, cancel, create, get_one,
    list, pause_lane, resume_lane, validate,
};
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

/// Due in an hour, so the inline wake after insert cannot claim it mid-test.
fn n8n(session_id: &str, source_id: &str) -> CreateEvent {
    req(json!({
        "session_id": session_id,
        "source": "n8n",
        "source_id": source_id,
        "body": "ticket updated",
        "not_before": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
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
    assert_eq!(valid.target, Target::Session("s1".into()));
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
fn a_target_is_a_session_or_a_preset_with_a_topic() {
    let preset = |extra: serde_json::Value| {
        let mut v = json!({"preset": " nightly ", "source": "n8n", "source_id": "a", "body": "x"});
        v.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        validate(req(v), now()).map(|valid| valid.target)
    };
    assert_eq!(
        preset(json!({})),
        Ok(Target::Preset { preset: "nightly".into(), topic: DEFAULT_TOPIC.into() })
    );
    assert_eq!(
        preset(json!({"topic": " ci "})),
        Ok(Target::Preset { preset: "nightly".into(), topic: "ci".into() })
    );
    assert!(preset(json!({"topic": "t".repeat(121)})).unwrap_err().contains("topic"));
    assert!(preset(json!({"session_id": "s1"})).unwrap_err().contains("not both"));
    let err = rejection(json!({"source": "n8n", "source_id": "a", "body": "x"}));
    assert!(err.contains("session_id or preset is required"), "{err}");
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

async fn seed_preset(pool: &sqlx::PgPool, user: Uuid, external_sources: bool) -> (Uuid, String) {
    let machine: Uuid = sqlx::query_scalar("SELECT id FROM machines WHERE user_id = $1 LIMIT 1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap();
    let name = format!("preset-{}", Uuid::new_v4());
    let id = sqlx::query_scalar(
        "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id, \
             permission_mode, external_sources) \
         VALUES ($1, $2, 'triage', $3, '/w', 'claude-code', 'ask', $4) RETURNING id",
    )
    .bind(user)
    .bind(&name)
    .bind(machine)
    .bind(external_sources)
    .fetch_one(pool)
    .await
    .unwrap();
    (id, name)
}

fn to_preset(preset: &str, source: &str, topic: Option<&str>) -> CreateEvent {
    req(json!({
        "preset": preset,
        "topic": topic,
        "source": source,
        "source_id": Uuid::new_v4().to_string(),
        "body": "ticket updated",
        "not_before": (Utc::now() + chrono::Duration::hours(1)).to_rfc3339(),
    }))
}

#[tokio::test]
async fn a_preset_event_lands_in_its_topic_lane_and_external_sources_need_consent() {
    let name = "inbox_route_preset_target";
    let Some(state) = state(name).await else { return };
    let (owner, _) = seed_session(&state.pool, name).await;
    let (stranger, _) = seed_session(&state.pool, name).await;
    let (open, open_name) = seed_preset(&state.pool, owner, true).await;
    let (closed, _) = seed_preset(&state.pool, owner, false).await;

    let (_, Json(by_name)) =
        post(&state, owner, to_preset(&open_name, "n8n", Some("ci"))).await.unwrap();
    let (_, Json(by_id)) =
        post(&state, owner, to_preset(&open.to_string(), "n8n", None)).await.unwrap();
    let lanes: Vec<(Uuid, String, Option<Uuid>)> = sqlx::query_as(
        "SELECT e.id, l.key, l.preset_id \
         FROM inbox_events e JOIN inbox_lanes l ON l.id = e.lane_id \
         WHERE e.id = ANY($1) ORDER BY l.key",
    )
    .bind([by_name.id, by_id.id].as_slice())
    .fetch_all(&state.pool)
    .await
    .unwrap();
    assert_eq!(
        lanes,
        [
            (by_name.id, format!("preset:{open}:ci"), Some(open)),
            (by_id.id, format!("preset:{open}:{DEFAULT_TOPIC}"), Some(open)),
        ]
    );

    let err =
        post(&state, owner, to_preset(&closed.to_string(), "webhook", None)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
    let (code, Json(internal)) =
        post(&state, owner, to_preset(&closed.to_string(), "timer", None)).await.unwrap();
    assert_eq!((code, internal.status.as_str()), (StatusCode::ACCEPTED, "accepted"));
    let err = post(&state, stranger, to_preset(&open.to_string(), "n8n", None)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
}

async fn lane_paused(pool: &sqlx::PgPool, lane: Uuid) -> (bool, Option<String>) {
    sqlx::query_as("SELECT paused, paused_reason FROM inbox_lanes WHERE id = $1")
        .bind(lane)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn pause_and_resume_are_owner_scoped_and_need_the_dispatch_scope() {
    let name = "inbox_route_pause_resume";
    let Some(state) = state(name).await else { return };
    let (owner, sid) = seed_session(&state.pool, name).await;
    let (stranger, _) = seed_session(&state.pool, name).await;
    let lane = crate::inbox::store::session_lane(&state.pool, owner, &sid).await.unwrap();

    let err = pause_lane(State(state.clone()), Extension(caller(stranger)), Path(lane))
        .await
        .unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    assert_eq!(lane_paused(&state.pool, lane).await, (false, None));

    let read_only = AuthContext { scopes: std::iter::once(Scope::Read).collect(), ..caller(owner) };
    let err = pause_lane(State(state.clone()), Extension(read_only), Path(lane)).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);

    let Json(paused) =
        pause_lane(State(state.clone()), Extension(caller(owner)), Path(lane)).await.unwrap();
    assert!(paused.paused);
    assert_eq!(paused.paused_reason.as_deref(), Some(crate::inbox::guards::MANUAL));

    let err = resume_lane(State(state.clone()), Extension(caller(stranger)), Path(lane))
        .await
        .unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
    assert!(lane_paused(&state.pool, lane).await.0, "a stranger cannot resume it");

    let Json(resumed) =
        resume_lane(State(state.clone()), Extension(caller(owner)), Path(lane)).await.unwrap();
    assert_eq!((resumed.paused, resumed.paused_reason), (false, None));
    let restarted: bool = sqlx::query_scalar(
        "SELECT ceiling_from > now() - interval '1 minute' FROM inbox_lanes WHERE id = $1",
    )
    .bind(lane)
    .fetch_one(&state.pool)
    .await
    .unwrap();
    assert!(restarted, "resuming restarts the ceiling window");
    let err =
        pause_lane(State(state), Extension(caller(owner)), Path(Uuid::new_v4())).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::NOT_FOUND);
}
