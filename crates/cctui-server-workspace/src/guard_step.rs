//! Live mirror of each session's workflow-guard step, as the worker pod
//! reports it. The guard in the pod decides; this only stores the last
//! snapshot, broadcasts changes, and relays transition requests down.
//!
//! The in-process map is the fast path; the `guard_steps` table backs it so a
//! replica that missed the report still answers, and a forget reaches every
//! replica through `POST /internal/guard/forget`.

use std::sync::{Arc, OnceLock};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::guard_step::GuardTransitionRequest;
use cctui_proto::guard_step::GuardStepState;
use cctui_proto::ws::ServerEvent;
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use crate::auth::AuthContext;
use crate::bus::Bus;
use crate::error::AppError;
use crate::events::{self, Actor, Event, kind};
use crate::state::AppState;

fn mirror() -> &'static DashMap<String, GuardStepState> {
    static MIRROR: OnceLock<DashMap<String, GuardStepState>> = OnceLock::new();
    MIRROR.get_or_init(DashMap::new)
}

static DURABLE: OnceLock<Durable> = OnceLock::new();

/// Body of `POST /internal/guard/forget`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardForget {
    pub session_id: String,
}

/// Carries a local forget to the other replicas.
pub trait ForgetRelay: Send + Sync {
    fn relay(&self, forget: GuardForget);
}

/// The table behind the mirror and the replicas a forget is sent to.
pub struct Durable {
    pool: PgPool,
    peers: Option<Arc<dyn ForgetRelay>>,
}

impl Durable {
    #[must_use]
    pub const fn new(pool: PgPool, peers: Option<Arc<dyn ForgetRelay>>) -> Self {
        Self { pool, peers }
    }

    fn persist_later(&self, session_id: String, state: GuardStepState) {
        let pool = self.pool.clone();
        let reported_at = Utc::now();
        tokio::spawn(async move {
            if let Err(err) = save(&pool, &session_id, &state, reported_at).await {
                tracing::warn!(%err, %session_id, "guard step not stored");
            }
        });
    }

    fn forget(&self, session_id: &str) {
        forget_local(session_id);
        if let Some(peers) = &self.peers {
            peers.relay(GuardForget { session_id: session_id.to_owned() });
        }
        let pool = self.pool.clone();
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            if let Err(err) = delete(&pool, &session_id).await {
                tracing::warn!(%err, %session_id, "guard step not removed");
            }
        });
    }

    async fn lookup(&self, session_id: &str) -> Option<GuardStepState> {
        if let Some(state) = get(session_id) {
            return Some(state);
        }
        load(&self.pool, session_id)
            .await
            .map_err(|err| tracing::warn!(%err, %session_id, "guard step read failed"))
            .ok()
            .flatten()
    }
}

/// Back the mirror with the `guard_steps` table and send forgets to `peers`.
pub fn init(durable: Durable) {
    let _ = DURABLE.set(durable);
}

/// Store the pod's snapshot and tell every client watching the session.
pub fn record_and_broadcast(bus: &Bus, session_id: String, state: GuardStepState) {
    record(session_id.clone(), state.clone());
    if let Some(durable) = DURABLE.get() {
        durable.persist_later(session_id.clone(), state.clone());
    }
    bus.publish_server(ServerEvent::GuardStepState { session_id, state });
}

/// Peer-ingest half of [`record_and_broadcast`]: must not re-publish.
pub fn record(session_id: String, state: GuardStepState) {
    mirror().insert(session_id, state);
}

#[must_use]
pub fn get(session_id: &str) -> Option<GuardStepState> {
    mirror().get(session_id).map(|s| s.clone())
}

/// Drop the step here, in the table and on every peer replica.
pub fn forget(session_id: &str) {
    match DURABLE.get() {
        Some(durable) => durable.forget(session_id),
        None => forget_local(session_id),
    }
}

/// Drop the step from this replica only; the receiving end of a peer forget.
pub fn forget_local(session_id: &str) {
    mirror().remove(session_id);
}

/// The step, read through to the table on a miss.
pub async fn lookup(session_id: &str) -> Option<GuardStepState> {
    match DURABLE.get() {
        Some(durable) => durable.lookup(session_id).await,
        None => get(session_id),
    }
}

/// Upsert the step unless a newer report landed first or the session is over.
async fn save(
    pool: &PgPool,
    session_id: &str,
    state: &GuardStepState,
    reported_at: DateTime<Utc>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO guard_steps (session_id, state, reported_at)
         SELECT id, $2, $3 FROM sessions
          WHERE id = $1 AND ended_at IS NULL AND status <> 'archived'
         ON CONFLICT (session_id) DO UPDATE
            SET state = EXCLUDED.state, reported_at = EXCLUDED.reported_at
          WHERE guard_steps.reported_at <= EXCLUDED.reported_at",
    )
    .bind(session_id)
    .bind(sqlx::types::Json(state))
    .bind(reported_at)
    .execute(pool)
    .await?;
    Ok(())
}

async fn load(pool: &PgPool, session_id: &str) -> Result<Option<GuardStepState>, sqlx::Error> {
    let row: Option<sqlx::types::Json<GuardStepState>> =
        sqlx::query_scalar("SELECT state FROM guard_steps WHERE session_id = $1")
            .bind(session_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(|json| json.0))
}

async fn delete(pool: &PgPool, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM guard_steps WHERE session_id = $1")
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// `GET /api/v1/sessions/{id}/guard` — the mirrored step, `null` for a
/// session whose pod runs no guard (or has not reported yet).
pub async fn guard_step(Path(session_id): Path<String>) -> Json<Option<GuardStepState>> {
    Json(lookup(&session_id).await)
}

/// `POST /api/v1/sessions/{id}/guard/transition` — ask the pod's guard to
/// move. Mints a `command_id`; the guard's verdict comes back as a
/// `CommandResult` and the new step as a `GuardStepState` frame.
pub async fn guard_transition(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<GuardTransitionRequest>,
) -> Result<(StatusCode, Json<cctui_proto::api::SpawnResponse>), AppError> {
    let step = req.step.trim().to_owned();
    if step.is_empty() || !(step.eq_ignore_ascii_case("exit") || step.parse::<u32>().is_ok()) {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "step must be a number or `exit`"));
    }
    let command_id = uuid::Uuid::new_v4();
    if let Err(err) = events::record_now(
        &state,
        Event::new(kind::SESSION_GUARD_TRANSITION, Actor::User(ctx.user_id))
            .session(&session_id)
            .detail(serde_json::json!({ "command_id": command_id, "step": step })),
    )
    .await
    {
        tracing::warn!(%err, session_id = %session_id, "lifecycle event insert failed");
    }
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.clone()),
        None,
    );
    let dispatch = crate::bus::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::GuardTransition {
            local_id: session_id.clone(),
            step: step.clone(),
            command_id: Some(command_id),
        },
    )
    .await;
    if let Err(err) = dispatch {
        use crate::bus::BusError;
        state.pending_commands.remove(&command_id);
        tracing::warn!(%session_id, %err, "guard transition dispatch failed");
        let (status, message) = match err {
            BusError::NotFound => (StatusCode::NOT_FOUND, err.to_string()),
            BusError::Timeout => (
                StatusCode::SERVICE_UNAVAILABLE,
                "the daemon is not reading its connection; the transition was not delivered"
                    .to_owned(),
            ),
            _ => (StatusCode::SERVICE_UNAVAILABLE, err.to_string()),
        };
        return Err(AppError::new(status, message));
    }
    tracing::info!(%session_id, %command_id, %step, "guard transition requested");
    Ok((
        StatusCode::ACCEPTED,
        Json(cctui_proto::api::SpawnResponse {
            command_id,
            status: "dispatched".into(),
            account: None,
            session_id: None,
        }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::NoopTransport;

    fn snapshot(step: i64) -> GuardStepState {
        GuardStepState {
            step,
            title: "Implement".into(),
            transitions: vec![3],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_report_is_stored_and_broadcast_to_the_session() {
        let bus = Bus::new(Box::new(NoopTransport));
        let mut rx = bus.subscribe_server();
        record_and_broadcast(&bus, "guard-s1".into(), snapshot(2));
        assert_eq!(get("guard-s1"), Some(snapshot(2)));

        let frame = rx.recv().await.unwrap();
        match frame.event.as_ref() {
            ServerEvent::GuardStepState { session_id, state } => {
                assert_eq!(session_id, "guard-s1");
                assert_eq!(state.step, 2);
                assert_eq!(state.transitions, vec![3]);
            }
            other => panic!("expected GuardStepState, got {other:?}"),
        }
        assert!(frame.json.contains(r#""type":"guard_step_state""#));

        record_and_broadcast(&bus, "guard-s1".into(), snapshot(3));
        assert_eq!(get("guard-s1").unwrap().step, 3, "the newest report wins");
        forget("guard-s1");
        assert_eq!(get("guard-s1"), None);
    }

    #[test]
    fn a_peer_report_is_stored() {
        record("guard-peer".into(), snapshot(4));
        assert_eq!(get("guard-peer"), Some(snapshot(4)));
        forget("guard-peer");
        assert_eq!(get("guard-peer"), None);
    }

    #[derive(Default)]
    struct RecordingPeers(std::sync::Mutex<Vec<GuardForget>>);

    impl ForgetRelay for RecordingPeers {
        fn relay(&self, forget: GuardForget) {
            self.0.lock().unwrap().push(forget);
        }
    }

    #[tokio::test]
    async fn a_forget_reaches_the_peers_and_a_peer_forget_does_not_loop_back() {
        let peers = Arc::new(RecordingPeers::default());
        let durable =
            Durable::new(PgPool::connect_lazy("postgres://invalid").unwrap(), Some(peers.clone()));
        record("guard-relay".into(), snapshot(2));
        durable.forget("guard-relay");
        assert_eq!(get("guard-relay"), None);
        assert_eq!(
            *peers.0.lock().unwrap(),
            vec![GuardForget { session_id: "guard-relay".into() }]
        );

        record("guard-relay".into(), snapshot(2));
        forget_local("guard-relay");
        assert_eq!(get("guard-relay"), None);
        assert_eq!(peers.0.lock().unwrap().len(), 1, "a peer's forget is not re-broadcast");
    }

    async fn db(test: &str) -> Option<(PgPool, String)> {
        let url = crate::routes::gateway::test_db_url(test)?;
        let pool = PgPool::connect(&url).await.expect("connect test db");
        let (uid, machine) = crate::events::tests::seed_user_machine(&pool, test).await;
        let sid = crate::events::tests::seed_session(&pool, uid, machine, Some("g")).await;
        Some((pool, sid))
    }

    #[tokio::test]
    async fn a_replica_that_missed_the_report_reads_the_table() {
        let Some((pool, sid)) = db("guard_step_read_through").await else { return };
        let durable = Durable::new(pool.clone(), None);
        let now = Utc::now();
        save(&pool, &sid, &snapshot(2), now).await.unwrap();
        assert_eq!(get(&sid), None);
        assert_eq!(durable.lookup(&sid).await, Some(snapshot(2)));

        save(&pool, &sid, &snapshot(1), now - chrono::Duration::seconds(5)).await.unwrap();
        assert_eq!(load(&pool, &sid).await.unwrap(), Some(snapshot(2)), "an older report loses");
        save(&pool, &sid, &snapshot(3), now + chrono::Duration::seconds(5)).await.unwrap();
        assert_eq!(load(&pool, &sid).await.unwrap(), Some(snapshot(3)));

        record(sid.clone(), snapshot(3));
        durable.forget(&sid);
        assert_eq!(get(&sid), None);
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while load(&pool, &sid).await.unwrap().is_some() {
            assert!(tokio::time::Instant::now() < deadline, "the forget never reached the table");
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(durable.lookup(&sid).await, None);
    }

    #[tokio::test]
    async fn an_ended_or_archived_session_stores_no_step() {
        let Some((pool, sid)) = db("guard_step_ended").await else { return };
        sqlx::query("UPDATE sessions SET ended_at = now() WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        save(&pool, &sid, &snapshot(2), Utc::now()).await.unwrap();
        assert_eq!(load(&pool, &sid).await.unwrap(), None);

        sqlx::query("UPDATE sessions SET ended_at = NULL, status = 'archived' WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        save(&pool, &sid, &snapshot(2), Utc::now()).await.unwrap();
        assert_eq!(load(&pool, &sid).await.unwrap(), None);
    }
}
