//! Pod-to-pod internal bus endpoints (CCT-573).
//!
//! `POST /internal/bus/route` delivers one forwarded frame/round-trip to a WS
//! held by THIS pod; `POST /internal/bus/publish` ingests a batch of relayed
//! [`crate::bus::BusEvent`]s into THIS pod's local subscribers and prompt
//! stores. Both are loop-guarded by construction: they only ever call the
//! bus's `*_local` delivery paths, so a forwarded frame or relayed event can
//! never be re-forwarded around the mesh.
//!
//! Auth: Bearer = the cluster-internal shared secret ([`ensure_secret`]),
//! minted once into `cluster_secrets` at first boot and read by every replica.
//! Constant-time comparison; user/machine/admin tokens are never accepted.
//! When this pod runs without the peer transport (`CCTUI_POD_IP` unset) no
//! secret is loaded and the endpoints refuse everything.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::bus::peer::{RouteRequest, RouteResponse, WireBusEvent, encode_error};
use crate::bus::{BusError, DaemonRequest, DaemonResponse};
use crate::routes::permissions::{PendingAsk, PendingPermission, PendingPlan};
use crate::state::AppState;

/// Row name of the internal bus secret in `cluster_secrets`.
const SECRET_NAME: &str = "internal_bus";

/// Load — minting on first boot — the cluster-internal shared secret. The
/// `INSERT ... ON CONFLICT DO NOTHING` makes the mint race-free across
/// replicas booting simultaneously: exactly one candidate wins and everyone
/// reads the winner back. Called only when the peer transport is enabled
/// (`CCTUI_POD_IP` set), so a single-replica/dev boot writes nothing.
pub async fn ensure_secret(pool: &PgPool) -> Result<String, sqlx::Error> {
    // Two v4 UUIDs = ~244 bits of CSPRNG entropy, hex-encoded — comfortably
    // past the 32-byte bar without pulling in a new RNG dependency.
    let candidate = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
    sqlx::query("INSERT INTO cluster_secrets (name, value) VALUES ($1, $2) ON CONFLICT DO NOTHING")
        .bind(SECRET_NAME)
        .bind(&candidate)
        .execute(pool)
        .await?;
    sqlx::query_scalar("SELECT value FROM cluster_secrets WHERE name = $1")
        .bind(SECRET_NAME)
        .fetch_one(pool)
        .await
}

/// Constant-time string equality via fixed-size digest comparison: hashing
/// both sides first makes the XOR-fold independent of where the inputs differ
/// and of their lengths.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let da = Sha256::digest(a.as_bytes());
    let db = Sha256::digest(b.as_bytes());
    da.iter().zip(db.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Gate an internal request on the shared secret. No secret loaded (peer
/// transport disabled) ⇒ 404, indistinguishable from the route not existing;
/// wrong/missing bearer ⇒ 401.
fn authenticate(state: &AppState, headers: &HeaderMap) -> Result<(), StatusCode> {
    let Some(secret) = state.internal_secret.as_deref() else {
        return Err(StatusCode::NOT_FOUND);
    };
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if constant_time_eq(bearer, secret) { Ok(()) } else { Err(StatusCode::UNAUTHORIZED) }
}

/// `POST /internal/bus/route` — deliver one forwarded frame/round-trip to a WS
/// terminated by THIS pod. Local delivery only (`*_local`): if the target
/// isn't here (it moved, or the presence row was stale) the miss is reported
/// in the body and the sending pod surfaces it honestly — one hop, never a
/// re-forward.
pub async fn bus_route(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(request): Json<RouteRequest>,
) -> Result<Json<RouteResponse>, StatusCode> {
    authenticate(&state, &headers)?;
    let outcome: Result<RouteResponse, BusError> = match request {
        RouteRequest::DaemonCommand { machine, frame } => {
            state.bus.command_daemon_local(machine, frame).await.map(|()| RouteResponse::Ok)
        }
        RouteRequest::DaemonStageFiles { machine, adapter_id, local_id, uploads } => state
            .bus
            .request_daemon_local(
                machine,
                DaemonRequest::StageFiles { adapter_id, local_id, uploads },
            )
            .await
            .map(|response| match response {
                DaemonResponse::StagedFiles(paths) => RouteResponse::StagedFiles { paths },
                DaemonResponse::Dirs(dirs) => RouteResponse::Dirs { dirs },
                DaemonResponse::Diagnose(report) => RouteResponse::Diagnose { report },
            }),
        RouteRequest::DaemonListDirs { machine, path } => {
            state.bus.request_daemon_local(machine, DaemonRequest::ListDirs { path }).await.map(
                |response| match response {
                    DaemonResponse::Dirs(dirs) => RouteResponse::Dirs { dirs },
                    DaemonResponse::StagedFiles(paths) => RouteResponse::StagedFiles { paths },
                    DaemonResponse::Diagnose(report) => RouteResponse::Diagnose { report },
                },
            )
        }
        RouteRequest::DaemonDiagnose { machine, adapter_id, local_id } => state
            .bus
            .request_daemon_local(machine, DaemonRequest::Diagnose { adapter_id, local_id })
            .await
            .map(|response| match response {
                DaemonResponse::Diagnose(report) => RouteResponse::Diagnose { report },
                DaemonResponse::StagedFiles(paths) => RouteResponse::StagedFiles { paths },
                DaemonResponse::Dirs(dirs) => RouteResponse::Dirs { dirs },
            }),
        RouteRequest::DispatcherCommand { dispatcher, frame } => {
            state.bus.command_dispatcher_local(dispatcher, frame).await.map(|()| RouteResponse::Ok)
        }
        RouteRequest::DispatcherRequest { dispatcher, request_id, frame } => state
            .bus
            .request_dispatcher_local(dispatcher, request_id, frame)
            .await
            .map(|frame| RouteResponse::DispatcherReply { frame }),
    };
    Ok(Json(outcome.unwrap_or_else(|err| {
        let (code, message) = encode_error(&err);
        RouteResponse::Err { code, message }
    })))
}

/// `POST /internal/bus/publish` — ingest a batch of relayed events into THIS
/// pod's local subscribers. `ServerEvent`s that carry prompt lifecycle
/// (permission/ask/plan raise + resolve) are also applied to the local stores
/// first, so replay-on-subscribe works on any pod (CCT-573 §store sync).
pub async fn bus_publish(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(events): Json<Vec<WireBusEvent>>,
) -> Result<StatusCode, StatusCode> {
    authenticate(&state, &headers)?;
    for event in events {
        if let WireBusEvent::Server { event } = &event {
            apply_server_event(&state, event).await;
        }
        state.bus.deliver_local(event.into());
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Mirror a relayed [`ServerEvent`]'s prompt-store side effects locally.
/// The originating pod already ran the daemon-ingest path (which wrote its own
/// stores and published these events); applying the same mutations here makes
/// both pods' stores converge — raise AND resolve — so a client (re)subscribing
/// on either pod gets the live prompt replayed.
async fn apply_server_event(state: &AppState, event: &cctui_proto::ws::ServerEvent) {
    use cctui_proto::ws::ServerEvent;
    match event {
        ServerEvent::PermissionRequest {
            session_id,
            request_id,
            tool_name,
            description,
            input_preview,
        } => {
            state.permission_store.write().await.insert_request(PendingPermission {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                tool_name: tool_name.clone(),
                description: description.clone(),
                input_preview: input_preview.clone(),
                received_at: Utc::now(),
            });
        }
        ServerEvent::PermissionResolved { request_id, .. } => {
            // Same terminal write the daemon-ingest resolution path performs.
            state.permission_store.write().await.record_decision(request_id, "resolved".into());
        }
        ServerEvent::AskQuestion { session_id, question, questions, preamble } => {
            state.permission_store.write().await.insert_ask(PendingAsk {
                session_id: session_id.clone(),
                question: question.clone(),
                questions: questions.clone(),
                preamble: preamble.clone(),
                received_at: Utc::now(),
            });
        }
        ServerEvent::AskResolved { session_id } => {
            state.permission_store.write().await.remove_ask(session_id);
        }
        ServerEvent::PlanRequest { session_id, plan, preamble } => {
            state.permission_store.write().await.insert_plan(PendingPlan {
                session_id: session_id.clone(),
                plan: plan.clone(),
                preamble: preamble.clone(),
                received_at: Utc::now(),
            });
        }
        ServerEvent::PlanResolved { session_id } => {
            state.permission_store.write().await.remove_plan(session_id);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn constant_time_eq_matches_semantics() {
        assert!(constant_time_eq("secret", "secret"));
        assert!(!constant_time_eq("secret", "secreT"));
        assert!(!constant_time_eq("secret", "secret-longer"));
        assert!(!constant_time_eq("", "x"));
        assert!(constant_time_eq("", ""));
    }
}
