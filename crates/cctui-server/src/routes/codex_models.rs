//! `GET /machines/{machine_id}/codex-models`.
//!
//! Returns the machine/account-scoped codex model catalog the daemon last
//! reported via `model/list`. Empty `models` when none is cached yet (daemon
//! offline, older daemon, or codex missing) — the webui then falls back to its
//! static offline list. Machine ownership is enforced by the `authz_layer`
//! guard (same as `fs::list_dirs`), so the handler only reads the cache.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cctui_proto::codex_catalog::CodexModelCatalog;
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;

pub async fn get_codex_models(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
) -> Result<Json<CodexModelCatalog>, AppError> {
    let machine_uuid = Uuid::parse_str(&machine_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))?;
    let catalog = state.codex_catalogs.get(&machine_uuid).map(|c| c.clone()).unwrap_or_default();
    Ok(Json(catalog))
}
