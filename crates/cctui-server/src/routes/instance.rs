//! Server-wide (instance) settings — admin-editable, everyone reads.
//!
//! `PUT /api/v1/admin/instance` sets the deployment name; reads come back on
//! `GET /api/v1/version` (`instance_name`) so the header needs a single query.
//! Operators running several cctui deployments (one per client) label each so
//! the header reads "cctui (NAME)" and the tab title "cctui (NAME)".
//!
//! The same table holds the **self-update target**: which enrolled machine
//! (and working directory / adapter) the "Update" button hands the deployment
//! to. It says nothing about *how* cctui is deployed there — Kubernetes,
//! Compose, a systemd unit — the agent spawned on that machine reads the local
//! instructions and does whatever this deployment normally does. Read/written
//! by `GET`/`PUT /api/v1/admin/instance/self-update`; `CCTUI_SELF_UPDATE_MACHINE`
//! (uuid or enrolled name) + `CCTUI_SELF_UPDATE_DIR` (+ optional
//! `CCTUI_SELF_UPDATE_ADAPTER`) are the config-file fallback when nothing is
//! stored.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::auth::{AuthContext, Scope};
use crate::state::AppState;
use cctui_proto::api::ApiError;

/// Hard cap on the label; it lives in a header slot and the tab title.
pub const NAME_MAX_CHARS: usize = 48;

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct InstanceUpdateRequest {
    /// New deployment name. Empty / whitespace-only clears it.
    pub name: Option<String>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct InstanceInfo {
    /// The deployment label, `null` when unset (the default).
    pub name: Option<String>,
}

/// Trim, collapse to `None` when empty, reject when over the cap.
fn normalize(raw: Option<&str>) -> Result<Option<String>, (StatusCode, Json<ApiError>)> {
    let Some(raw) = raw else { return Ok(None) };
    let trimmed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if trimmed.is_empty() {
        return Ok(None);
    }
    if trimmed.chars().count() > NAME_MAX_CHARS {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: format!("name must be at most {NAME_MAX_CHARS} characters") }),
        ));
    }
    Ok(Some(trimmed))
}

/// The stored deployment name, `None` when unset or on a read error (the
/// header degrades to the bare brand rather than failing the version call).
pub async fn read_name(pool: &sqlx::PgPool) -> Option<String> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM instance_settings WHERE key = 'name'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .and_then(|v| v.as_str().map(str::to_owned))
    .filter(|s| !s.is_empty())
}

pub async fn update(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<InstanceUpdateRequest>,
) -> Result<Json<InstanceInfo>, (StatusCode, Json<ApiError>)> {
    ctx.requires(Scope::Admin)
        .map_err(|s| (s, Json(ApiError { error: "admin token required".into() })))?;
    let name = normalize(req.name.as_deref())?;
    let res = match &name {
        Some(n) => {
            sqlx::query(
                "INSERT INTO instance_settings (key, value, updated_at) VALUES ('name', to_jsonb($1::text), now()) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(n)
            .execute(&state.pool)
            .await
        }
        None => sqlx::query("DELETE FROM instance_settings WHERE key = 'name'").execute(&state.pool).await,
    };
    res.map_err(|e| {
        tracing::error!("instance settings write failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    Ok(Json(InstanceInfo { name }))
}

/// Where the self-update agent runs. Stored under `instance_settings.self_update`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SelfUpdateTarget {
    /// Enrolled machine (uuid) the update session is spawned on.
    pub machine_id: String,
    /// Working directory of that session; the deployment's checkout or
    /// operations folder, whatever the local instructions expect.
    pub working_dir: String,
    /// Adapter to run it under (`claude-code` / `codex`); `None` → claude-code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_id: Option<String>,
}

#[derive(Deserialize, TS)]
#[ts(export)]
pub struct SelfUpdateTargetRequest {
    /// `null` clears the stored target (the env fallback, if any, then applies).
    pub target: Option<SelfUpdateTarget>,
}

#[derive(Serialize, TS)]
#[ts(export)]
pub struct SelfUpdateTargetInfo {
    /// The effective target: stored one first, else the env fallback, else
    /// `null` (the button then tells the admin to configure one).
    pub target: Option<SelfUpdateTarget>,
    /// `"settings"` when it comes from the admin form, `"env"` from
    /// `CCTUI_SELF_UPDATE_*`, `null` when unset.
    pub source: Option<&'static str>,
}

/// The target stored by the admin form, `None` when unset or unreadable.
async fn stored_self_update_target(pool: &sqlx::PgPool) -> Option<SelfUpdateTarget> {
    sqlx::query_scalar::<_, serde_json::Value>(
        "SELECT value FROM instance_settings WHERE key = 'self_update'",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .and_then(|v| serde_json::from_value(v).ok())
}

/// `CCTUI_SELF_UPDATE_MACHINE` + `CCTUI_SELF_UPDATE_DIR` fallback; the machine
/// may be given as a uuid or as its enrolled name, resolved here so callers
/// always get a uuid.
async fn env_self_update_target(pool: &sqlx::PgPool) -> Option<SelfUpdateTarget> {
    let machine = std::env::var("CCTUI_SELF_UPDATE_MACHINE").ok()?.trim().to_owned();
    let working_dir = std::env::var("CCTUI_SELF_UPDATE_DIR").ok()?.trim().to_owned();
    if machine.is_empty() || working_dir.is_empty() {
        return None;
    }
    let adapter_id = std::env::var("CCTUI_SELF_UPDATE_ADAPTER")
        .ok()
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty());
    let machine_id = match uuid::Uuid::parse_str(&machine) {
        Ok(id) => id.to_string(),
        Err(_) => sqlx::query_scalar::<_, uuid::Uuid>(
            "SELECT id FROM machines WHERE (name = $1 OR display_name = $1) AND revoked_at IS NULL \
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(&machine)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()?
        .to_string(),
    };
    Some(SelfUpdateTarget { machine_id, working_dir, adapter_id })
}

/// The effective self-update target and where it comes from.
pub async fn read_self_update_target(pool: &sqlx::PgPool) -> SelfUpdateTargetInfo {
    if let Some(t) = stored_self_update_target(pool).await {
        return SelfUpdateTargetInfo { target: Some(t), source: Some("settings") };
    }
    if let Some(t) = env_self_update_target(pool).await {
        return SelfUpdateTargetInfo { target: Some(t), source: Some("env") };
    }
    SelfUpdateTargetInfo { target: None, source: None }
}

/// Trim both fields and require a uuid machine id + non-empty directory.
fn normalize_target(
    raw: SelfUpdateTarget,
) -> Result<SelfUpdateTarget, (StatusCode, Json<ApiError>)> {
    let bad = |msg: &str| (StatusCode::BAD_REQUEST, Json(ApiError { error: msg.into() }));
    let machine_id = uuid::Uuid::parse_str(raw.machine_id.trim())
        .map_err(|_| bad("machine_id must be a uuid"))?;
    let working_dir = raw.working_dir.trim().to_owned();
    if working_dir.is_empty() {
        return Err(bad("working_dir is required"));
    }
    let adapter_id = raw.adapter_id.map(|a| a.trim().to_owned()).filter(|a| !a.is_empty());
    if adapter_id.as_deref().is_some_and(|a| !matches!(a, "claude-code" | "codex")) {
        return Err(bad("adapter_id must be claude-code or codex"));
    }
    Ok(SelfUpdateTarget { machine_id: machine_id.to_string(), working_dir, adapter_id })
}

pub async fn get_self_update_target(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SelfUpdateTargetInfo>, (StatusCode, Json<ApiError>)> {
    ctx.requires(Scope::Admin)
        .map_err(|s| (s, Json(ApiError { error: "admin token required".into() })))?;
    Ok(Json(read_self_update_target(&state.pool).await))
}

pub async fn update_self_update_target(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<SelfUpdateTargetRequest>,
) -> Result<Json<SelfUpdateTargetInfo>, (StatusCode, Json<ApiError>)> {
    ctx.requires(Scope::Admin)
        .map_err(|s| (s, Json(ApiError { error: "admin token required".into() })))?;
    let db_err = |e: sqlx::Error| {
        tracing::error!("instance settings write failed: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    };
    match req.target {
        Some(raw) => {
            let target = normalize_target(raw)?;
            let exists: Option<(Uuid,)> =
                sqlx::query_as("SELECT id FROM machines WHERE id = $1 AND revoked_at IS NULL")
                    .bind(uuid::Uuid::parse_str(&target.machine_id).expect("normalized"))
                    .fetch_optional(&state.pool)
                    .await
                    .map_err(db_err)?;
            if exists.is_none() {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(ApiError { error: "machine not found".into() }),
                ));
            }
            sqlx::query(
                "INSERT INTO instance_settings (key, value, updated_at) VALUES ('self_update', $1, now()) \
                 ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value, updated_at = now()",
            )
            .bind(serde_json::to_value(&target).expect("serializable"))
            .execute(&state.pool)
            .await
            .map_err(db_err)?;
        }
        None => {
            sqlx::query("DELETE FROM instance_settings WHERE key = 'self_update'")
                .execute(&state.pool)
                .await
                .map_err(db_err)?;
        }
    }
    Ok(Json(read_self_update_target(&state.pool).await))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_target_validates() {
        let ok = normalize_target(SelfUpdateTarget {
            machine_id: format!(" {} ", Uuid::nil()),
            working_dir: " /srv/cctui ".into(),
            adapter_id: Some("  ".into()),
        })
        .unwrap();
        assert_eq!(ok.machine_id, Uuid::nil().to_string());
        assert_eq!(ok.working_dir, "/srv/cctui");
        assert_eq!(ok.adapter_id, None);
        let bad_id = normalize_target(SelfUpdateTarget {
            machine_id: "agents".into(),
            working_dir: "/x".into(),
            adapter_id: None,
        });
        assert_eq!(bad_id.unwrap_err().0, StatusCode::BAD_REQUEST);
        let bad_dir = normalize_target(SelfUpdateTarget {
            machine_id: Uuid::nil().to_string(),
            working_dir: " ".into(),
            adapter_id: None,
        });
        assert_eq!(bad_dir.unwrap_err().0, StatusCode::BAD_REQUEST);
        let bad_adapter = normalize_target(SelfUpdateTarget {
            machine_id: Uuid::nil().to_string(),
            working_dir: "/x".into(),
            adapter_id: Some("opencode".into()),
        });
        assert_eq!(bad_adapter.unwrap_err().0, StatusCode::BAD_REQUEST);
    }

    #[test]
    fn normalize_trims_and_clears() {
        assert_eq!(normalize(None).unwrap(), None);
        assert_eq!(normalize(Some("   ")).unwrap(), None);
        assert_eq!(normalize(Some("  Acme   Corp ")).unwrap(), Some("Acme Corp".into()));
        assert_eq!(normalize(Some("Été")).unwrap(), Some("Été".into()));
    }

    #[test]
    fn normalize_rejects_too_long() {
        let long = "x".repeat(NAME_MAX_CHARS + 1);
        assert_eq!(normalize(Some(&long)).unwrap_err().0, StatusCode::BAD_REQUEST);
        let ok = "x".repeat(NAME_MAX_CHARS);
        assert!(normalize(Some(&ok)).is_ok());
    }
}
