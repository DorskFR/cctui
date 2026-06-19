//! Working-directory autocomplete for the spawn dialog.
//!
//! `GET /machines/{machine_id}/fs/dirs?path=…` asks the machine's daemon for
//! the sub-directories of `path` and returns their names. The daemon answers
//! over its existing WS with the same request_id + oneshot pattern as
//! mid-chat file staging (CCT-236). Ownership rule matches spawn: the machine
//! must belong to the requesting user (admin tokens may browse any machine) —
//! no path restriction beyond that, since machine owners can already spawn
//! arbitrary commands.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::ApiError;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthContext;
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

pub async fn list_dirs(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(machine_id): Path<String>,
    Query(params): Query<ListDirsParams>,
) -> Result<Json<ListDirsResponse>, (StatusCode, Json<ApiError>)> {
    let machine_uuid = Uuid::parse_str(&machine_id).map_err(|_| {
        (StatusCode::BAD_REQUEST, Json(ApiError { error: "machine_id must be a uuid".into() }))
    })?;
    let row: Option<(Uuid,)> = sqlx::query_as("SELECT user_id FROM machines WHERE id = $1")
        .bind(machine_uuid)
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    let Some((owner,)) = row else {
        return Err((StatusCode::NOT_FOUND, Json(ApiError { error: "machine not found".into() })));
    };
    let permitted = ctx.is_admin() || ctx.user_id == owner;
    if !permitted {
        return Err((StatusCode::FORBIDDEN, Json(ApiError { error: "not your machine".into() })));
    }

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
