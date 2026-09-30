//! `GET /api/v1/models/{harness}` — the model and effort lists a picker for
//! that harness should offer.
//!
//! Codex is catalog-driven, so an optional `machine_id` narrows the catalog to
//! the one that machine reports; every other harness answers from the static
//! lists. There is no allowlist — the picker also accepts free text.

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use cctui_proto::harness_models::{HarnessModels, harness_models};
use serde::Deserialize;
use uuid::Uuid;

use crate::error::AppError;
use crate::routes::codex_models::effective_catalog;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct ModelsQuery {
    #[serde(default)]
    pub machine_id: Option<String>,
    /// The already-selected model, so `efforts` are the ones it supports.
    #[serde(default)]
    pub model: Option<String>,
}

pub async fn get_harness_models(
    State(state): State<AppState>,
    Path(harness): Path<String>,
    Query(query): Query<ModelsQuery>,
) -> Result<Json<HarnessModels>, AppError> {
    let machine = match query.machine_id.as_deref().filter(|m| !m.is_empty()) {
        Some(raw) => Some(
            Uuid::parse_str(raw)
                .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))?,
        ),
        None => None,
    };
    let catalog = (harness == "codex").then(|| effective_catalog(&state, machine));
    let model = query.model.as_deref().unwrap_or_default();
    Ok(Json(harness_models(&harness, catalog.as_ref(), model)))
}
