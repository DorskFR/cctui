//! Server-wide (instance) settings — admin-editable, everyone reads.
//!
//! `PUT /api/v1/admin/instance` sets the deployment name; reads come back on
//! `GET /api/v1/version` (`instance_name`) so the header needs a single query.
//! Operators running several cctui deployments (one per client) label each so
//! the header reads "cctui (NAME)" and the tab title "cctui (NAME)".

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::auth::{AuthContext, Scope};
use cctui_proto::api::ApiError;
use crate::state::AppState;

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
    sqlx::query_scalar::<_, serde_json::Value>("SELECT value FROM instance_settings WHERE key = 'name'")
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

#[cfg(test)]
mod tests {
    use super::*;

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
