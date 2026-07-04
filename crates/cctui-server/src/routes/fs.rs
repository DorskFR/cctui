//! Working-directory autocomplete for the spawn dialog.
//!
//! `GET /machines/{machine_id}/fs/dirs?path=…` asks the machine's daemon for
//! the sub-directories of `path` and returns their names. The daemon answers
//! over its existing WS with the same `request_id` + oneshot pattern as
//! mid-chat file staging (CCT-236). Ownership rule matches spawn: the machine
//! must belong to the requesting user (admin tokens may browse any machine) —
//! no path restriction beyond that, since machine owners can already spawn
//! arbitrary commands.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use cctui_proto::api::ApiError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::daemon_dispatch;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ListDirsParams {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct ListDirsResponse {
    pub dirs: Vec<String>,
}

// Machine ownership is enforced by the `Resource(Machine, Read, IdFrom::Path
// ("machine_id"))` guard in `authz.rs` (CCT-420): the `authz_layer` middleware
// resolves `machines.user_id` and applies `admin || owner == caller` BEFORE this
// handler runs (404 unknown machine / 403 not-your-machine / admin bypass — the
// exact semantics of the old in-handler check). The handler now only needs the
// machine id to talk to the daemon.
pub async fn list_dirs(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    Query(params): Query<ListDirsParams>,
) -> Result<Json<ListDirsResponse>, (StatusCode, Json<ApiError>)> {
    let machine_uuid = Uuid::parse_str(&machine_id).map_err(|_| {
        (StatusCode::BAD_REQUEST, Json(ApiError { error: "machine_id must be a uuid".into() }))
    })?;

    // Forward to the replica holding this machine's daemon WS (CCT-567).
    crate::forward::ensure_daemon_local(&state, machine_uuid).await?;

    match daemon_dispatch::list_dirs(&state, machine_uuid, params.path).await {
        Ok(dirs) => Ok(Json(ListDirsResponse { dirs })),
        Err(daemon_dispatch::Error::NoDaemon(_)) => Err((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ApiError { error: "daemon offline".into() }),
        )),
        Err(daemon_dispatch::Error::Timeout) => Err((
            StatusCode::GATEWAY_TIMEOUT,
            Json(ApiError { error: "timed out waiting for the daemon".into() }),
        )),
        Err(daemon_dispatch::Error::ListDirs(msg)) => {
            Err((StatusCode::BAD_REQUEST, Json(ApiError { error: msg })))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: e.to_string() }))),
    }
}
