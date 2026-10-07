//! `/api/v1/machines/{machine_id}/adapters` — which harnesses a machine runs.
//!
//! Reads are for the machine's owner (the spawn picker narrows to them);
//! writes are admin-only and push a fresh Reconcile so the daemon converges
//! without a reconnect. Running sessions are never touched.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::state::AppState;

pub use cctui_proto::api::machine_adapters::{MachineAdapterInfo, SetMachineAdapterRequest};

type ApiResult<T> = Result<Json<T>, AppError>;

fn admin(ctx: &AuthContext) -> Result<(), AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))
}

/// The machine's view of the harness table: every table row, plus any row
/// the machine pins for a harness the table does not know.
fn merge(rows: Vec<(String, serde_json::Value, bool)>) -> Vec<MachineAdapterInfo> {
    let mut out: Vec<MachineAdapterInfo> = cctui_proto::adapter::harnesses()
        .into_iter()
        .map(|h| {
            let row = rows.iter().find(|(id, _, _)| *id == h.id);
            MachineAdapterInfo {
                adapter_id: h.id,
                enabled: row.map_or(h.default_enabled, |(_, _, enabled)| *enabled),
                config: row.map_or_else(|| serde_json::json!({}), |(_, config, _)| config.clone()),
                pinned: row.is_some(),
                default_enabled: h.default_enabled,
            }
        })
        .collect();
    for (id, config, enabled) in rows {
        if !out.iter().any(|a| a.adapter_id == id) {
            out.push(MachineAdapterInfo {
                adapter_id: id,
                enabled,
                config,
                pinned: true,
                default_enabled: false,
            });
        }
    }
    out
}

async fn rows(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
) -> Result<Vec<(String, serde_json::Value, bool)>, AppError> {
    Ok(sqlx::query_as(
        "SELECT adapter_id, config, enabled FROM adapters_enabled WHERE machine_id = $1",
    )
    .bind(machine_id)
    .fetch_all(pool)
    .await?)
}

async fn machine_exists(pool: &sqlx::PgPool, machine_id: Uuid) -> Result<(), AppError> {
    let found: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND revoked_at IS NULL)",
    )
    .bind(machine_id)
    .fetch_one(pool)
    .await?;
    if found { Ok(()) } else { Err(AppError::new(StatusCode::NOT_FOUND, "machine not found")) }
}

async fn read_all(state: &AppState, machine_id: Uuid) -> ApiResult<Vec<MachineAdapterInfo>> {
    machine_exists(&state.pool, machine_id).await?;
    Ok(Json(merge(rows(&state.pool, machine_id).await?)))
}

fn clean_adapter(adapter: &str) -> Result<String, AppError> {
    let id = adapter.trim();
    let ok = !id.is_empty()
        && id.len() <= 64
        && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(id.to_owned())
    } else {
        Err(AppError::new(StatusCode::BAD_REQUEST, "adapter id must be lowercase [a-z0-9-]"))
    }
}

fn clean_config(config: serde_json::Value) -> Result<serde_json::Value, AppError> {
    if config.is_object() {
        Ok(config)
    } else {
        Err(AppError::new(StatusCode::BAD_REQUEST, "config must be a JSON object"))
    }
}

async fn push(state: &AppState, machine_id: Uuid) {
    if let Err(e) = crate::bus::push_reconcile(state, machine_id).await {
        tracing::debug!(%machine_id, error = %e, "adapter change: no live daemon to reconcile");
    }
}

pub async fn list(
    State(state): State<AppState>,
    Path(machine_id): Path<Uuid>,
) -> ApiResult<Vec<MachineAdapterInfo>> {
    read_all(&state, machine_id).await
}

pub async fn set(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((machine_id, adapter)): Path<(Uuid, String)>,
    Json(req): Json<SetMachineAdapterRequest>,
) -> ApiResult<Vec<MachineAdapterInfo>> {
    admin(&ctx)?;
    machine_exists(&state.pool, machine_id).await?;
    let adapter = clean_adapter(&adapter)?;
    let config = req.config.map(clean_config).transpose()?;
    sqlx::query(
        "INSERT INTO adapters_enabled (machine_id, adapter_id, config, enabled, updated_at) \
         VALUES ($1, $2, COALESCE($3, '{}'::jsonb), COALESCE($4, TRUE), now()) \
         ON CONFLICT (machine_id, adapter_id) DO UPDATE SET \
            config = COALESCE(EXCLUDED.config, adapters_enabled.config), \
            enabled = COALESCE($4, adapters_enabled.enabled), \
            updated_at = now()",
    )
    .bind(machine_id)
    .bind(&adapter)
    .bind(config)
    .bind(req.enabled)
    .execute(&state.pool)
    .await?;
    tracing::info!(
        %machine_id,
        adapter,
        enabled = ?req.enabled,
        by = ?ctx.user_id,
        "machine adapter set"
    );
    push(&state, machine_id).await;
    read_all(&state, machine_id).await
}

pub async fn reset(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((machine_id, adapter)): Path<(Uuid, String)>,
) -> ApiResult<Vec<MachineAdapterInfo>> {
    admin(&ctx)?;
    machine_exists(&state.pool, machine_id).await?;
    let adapter = clean_adapter(&adapter)?;
    sqlx::query("DELETE FROM adapters_enabled WHERE machine_id = $1 AND adapter_id = $2")
        .bind(machine_id)
        .bind(&adapter)
        .execute(&state.pool)
        .await?;
    tracing::info!(%machine_id, adapter, by = ?ctx.user_id, "machine adapter reset to default");
    push(&state, machine_id).await;
    read_all(&state, machine_id).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn without_rows_every_harness_reports_its_default() {
        let got = merge(Vec::new());
        let ids: Vec<&str> = got.iter().map(|a| a.adapter_id.as_str()).collect();
        assert_eq!(ids, cctui_proto::adapter::KNOWN_ADAPTERS);
        assert!(got.iter().all(|a| a.enabled && !a.pinned && a.config == json!({})));
    }

    #[test]
    fn a_row_pins_the_harness_and_an_unknown_row_is_listed_after_the_table() {
        let got = merge(vec![
            ("opencode".to_owned(), json!({"bin": "/opt/opencode"}), false),
            ("gemini".to_owned(), json!({}), true),
        ]);
        let opencode = got.iter().find(|a| a.adapter_id == "opencode").unwrap();
        assert!(opencode.pinned && !opencode.enabled && opencode.default_enabled);
        assert_eq!(opencode.config, json!({"bin": "/opt/opencode"}));
        let last = got.last().unwrap();
        assert_eq!(last.adapter_id, "gemini");
        assert!(last.pinned && last.enabled && !last.default_enabled);
    }

    #[test]
    fn adapter_ids_are_lowercase_slugs_and_configs_are_objects() {
        assert_eq!(clean_adapter(" codex ").unwrap(), "codex");
        assert!(clean_adapter("Codex").is_err());
        assert!(clean_adapter("").is_err());
        assert!(clean_adapter("a/b").is_err());
        assert!(clean_config(json!({"bin": "x"})).is_ok());
        assert!(clean_config(json!(["x"])).is_err());
        assert!(clean_config(json!(null)).is_err());
    }
}
