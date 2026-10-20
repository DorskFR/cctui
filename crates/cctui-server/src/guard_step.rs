//! Live mirror of each session's workflow-guard step, as the worker pod
//! reports it. The guard in the pod decides; this only stores the last
//! snapshot, broadcasts changes, and relays transition requests down.

use std::sync::OnceLock;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::guard_step::GuardTransitionRequest;
use cctui_proto::guard_step::GuardStepState;
use cctui_proto::ws::ServerEvent;
use dashmap::DashMap;

use crate::auth::AuthContext;
use crate::bus::Bus;
use crate::error::AppError;
use crate::events::{self, Actor, Event, kind};
use crate::state::AppState;

fn mirror() -> &'static DashMap<String, GuardStepState> {
    static MIRROR: OnceLock<DashMap<String, GuardStepState>> = OnceLock::new();
    MIRROR.get_or_init(DashMap::new)
}

/// Store the pod's snapshot and tell every client watching the session.
pub fn record_and_broadcast(bus: &Bus, session_id: String, state: GuardStepState) {
    mirror().insert(session_id.clone(), state.clone());
    bus.publish_server(ServerEvent::GuardStepState { session_id, state });
}

#[must_use]
pub fn get(session_id: &str) -> Option<GuardStepState> {
    mirror().get(session_id).map(|s| s.clone())
}

pub fn forget(session_id: &str) {
    mirror().remove(session_id);
}

/// `GET /api/v1/sessions/{id}/guard` — the mirrored step, `null` for a
/// session whose pod runs no guard (or has not reported yet).
#[allow(clippy::unused_async)]
pub async fn guard_step(Path(session_id): Path<String>) -> Json<Option<GuardStepState>> {
    Json(get(&session_id))
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
}
