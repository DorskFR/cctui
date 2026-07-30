//! Working-directory autocomplete for the spawn dialog.
//!
//! `GET /machines/{machine_id}/fs/dirs?path=…` asks the machine's daemon for
//! the sub-directories of `path` and returns their names. The daemon answers
//! over its existing WS with the same `request_id` + oneshot pattern as
//! mid-chat file staging. Ownership rule matches spawn: the machine
//! must belong to the requesting user (admin tokens may browse any machine) —
//! no path restriction beyond that, since machine owners can already spawn
//! arbitrary commands.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::bus;
use crate::error::AppError;
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
// ("machine_id"))` guard in `authz.rs`: the `authz_layer` middleware
// resolves `machines.user_id` and applies `admin || owner == caller` before this
// handler runs (404 unknown machine / 403 not-your-machine / admin bypass). The
// handler only needs the machine id to talk to the daemon.
pub async fn list_dirs(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
    Query(params): Query<ListDirsParams>,
) -> Result<Json<ListDirsResponse>, AppError> {
    let machine_uuid = Uuid::parse_str(&machine_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))?;

    match bus::list_dirs(&state, machine_uuid, params.path).await {
        Ok(dirs) => Ok(Json(ListDirsResponse { dirs })),
        Err(bus::BusError::NoDaemon(_)) => {
            Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "daemon offline"))
        }
        Err(bus::BusError::Timeout) => {
            Err(AppError::new(StatusCode::GATEWAY_TIMEOUT, "timed out waiting for the daemon"))
        }
        Err(bus::BusError::ListDirs(msg)) => Err(AppError::new(StatusCode::BAD_REQUEST, msg)),
        Err(e) => Err(AppError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}
