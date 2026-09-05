//! Codex model catalogs reported by daemons via `model/list`.
//!
//! `GET /machines/{machine_id}/codex-models` returns one machine's catalog
//! (empty `models` when none is known — daemon offline, older daemon, codex
//! missing — and the webui falls back to its static offline list).
//! `GET /models/codex` merges every machine's catalog for pickers with no
//! machine in hand (dispatch, fork): a union by model id, newest report wins.
//! `POST /machines/{machine_id}/codex-models/refresh` asks the daemon to
//! re-run `model/list` without spawning a session.
//! Catalogs persist in `codex_model_catalogs`, warmed into
//! `AppState::codex_catalogs` on boot. Machine ownership is enforced by the
//! `authz_layer` guard (same as `fs::list_dirs`).

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use cctui_proto::codex_catalog::{CodexModel, CodexModelCatalog};
use cctui_proto::ws::DaemonFrameDown;
use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

use crate::bus;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Debug, Clone)]
pub struct CachedCatalog {
    pub catalog: CodexModelCatalog,
    pub fetched_at: DateTime<Utc>,
}

/// The merged cross-machine view: model ids from every machine, each taken
/// from the most recently fetched catalog that lists it.
#[derive(Debug, Default, Serialize)]
pub struct MergedCodexCatalog {
    pub models: Vec<CodexModel>,
    pub fetched_at: Option<DateTime<Utc>>,
    pub machines: usize,
}

pub fn merge_catalogs<'a>(
    catalogs: impl IntoIterator<Item = &'a CachedCatalog>,
) -> MergedCodexCatalog {
    let mut sorted: Vec<&CachedCatalog> = catalogs.into_iter().collect();
    sorted.sort_by_key(|c| std::cmp::Reverse(c.fetched_at));
    let mut merged = MergedCodexCatalog { machines: sorted.len(), ..Default::default() };
    let mut seen = std::collections::HashSet::new();
    for cached in sorted {
        merged.fetched_at.get_or_insert(cached.fetched_at);
        for model in &cached.catalog.models {
            if seen.insert(model.id.clone()) {
                merged.models.push(model.clone());
            }
        }
    }
    merged
}

pub async fn store_catalog(state: &AppState, machine_id: Uuid, catalog: CodexModelCatalog) {
    let fetched_at = Utc::now();
    let json = serde_json::to_value(&catalog).unwrap_or(serde_json::Value::Null);
    state.codex_catalogs.insert(machine_id, CachedCatalog { catalog, fetched_at });
    if let Err(err) = sqlx::query(
        "INSERT INTO codex_model_catalogs (machine_id, catalog, fetched_at) VALUES ($1, $2, $3) \
         ON CONFLICT (machine_id) DO UPDATE SET catalog = EXCLUDED.catalog, fetched_at = EXCLUDED.fetched_at",
    )
    .bind(machine_id)
    .bind(json)
    .bind(fetched_at)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%machine_id, %err, "failed to persist codex model catalog");
    }
}

pub async fn warm_cache(state: &AppState) {
    let rows: Vec<(Uuid, serde_json::Value, DateTime<Utc>)> =
        match sqlx::query_as("SELECT machine_id, catalog, fetched_at FROM codex_model_catalogs")
            .fetch_all(&state.pool)
            .await
        {
            Ok(rows) => rows,
            Err(err) => {
                tracing::warn!(%err, "failed to load codex model catalogs");
                return;
            }
        };
    for (machine_id, json, fetched_at) in rows {
        match serde_json::from_value::<CodexModelCatalog>(json) {
            Ok(catalog) => {
                state.codex_catalogs.insert(machine_id, CachedCatalog { catalog, fetched_at });
            }
            Err(err) => tracing::warn!(%machine_id, %err, "malformed persisted codex catalog"),
        }
    }
}

fn parse_machine(machine_id: &str) -> Result<Uuid, AppError> {
    Uuid::parse_str(machine_id)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "machine_id must be a uuid"))
}

pub async fn get_codex_models(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
) -> Result<Json<CodexModelCatalog>, AppError> {
    let machine_uuid = parse_machine(&machine_id)?;
    let catalog =
        state.codex_catalogs.get(&machine_uuid).map(|c| c.catalog.clone()).unwrap_or_default();
    Ok(Json(catalog))
}

pub async fn get_merged_codex_models(
    State(state): State<AppState>,
) -> Result<Json<MergedCodexCatalog>, AppError> {
    let cached: Vec<CachedCatalog> =
        state.codex_catalogs.iter().map(|c| c.value().clone()).collect();
    Ok(Json(merge_catalogs(&cached)))
}

pub async fn refresh_codex_models(
    State(state): State<AppState>,
    Path(machine_id): Path<String>,
) -> Result<StatusCode, AppError> {
    let machine_uuid = parse_machine(&machine_id)?;
    match state.bus.command_daemon(machine_uuid, DaemonFrameDown::RefreshCodexModels {}).await {
        Ok(()) => Ok(StatusCode::ACCEPTED),
        Err(bus::BusError::NoDaemon(_)) => {
            Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "daemon offline"))
        }
        Err(e) => Err(AppError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, display_name: &str) -> CodexModel {
        CodexModel {
            id: id.to_owned(),
            model: id.to_owned(),
            display_name: display_name.to_owned(),
            description: String::new(),
            hidden: false,
            is_default: false,
            supported_efforts: vec![],
            default_effort: String::new(),
            input_modalities: vec![],
            upgrade: None,
        }
    }

    fn cached(models: Vec<CodexModel>, secs: i64) -> CachedCatalog {
        CachedCatalog {
            catalog: CodexModelCatalog { models },
            fetched_at: DateTime::from_timestamp(secs, 0).unwrap(),
        }
    }

    #[test]
    fn merge_is_a_union_where_the_newest_report_wins() {
        let old = cached(vec![model("gpt-a", "A old"), model("gpt-old-only", "Old only")], 10);
        let new = cached(vec![model("gpt-a", "A new"), model("gpt-b", "B")], 20);
        let merged = merge_catalogs([&old, &new]);
        let labels: Vec<(&str, &str)> =
            merged.models.iter().map(|m| (m.id.as_str(), m.display_name.as_str())).collect();
        assert_eq!(labels, [("gpt-a", "A new"), ("gpt-b", "B"), ("gpt-old-only", "Old only")]);
        assert_eq!(merged.fetched_at, Some(new.fetched_at));
        assert_eq!(merged.machines, 2);
    }

    #[test]
    fn merge_of_nothing_is_empty() {
        let merged = merge_catalogs([]);
        assert!(merged.models.is_empty());
        assert_eq!(merged.fetched_at, None);
        assert_eq!(merged.machines, 0);
    }
}
