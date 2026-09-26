//! Session API: list previews, mint access tickets. Owner only.

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use serde::Serialize;
use uuid::Uuid;

use super::Preview;
use crate::auth::AuthContext;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PreviewInfo {
    pub id: String,
    pub port: u16,
    pub url: String,
    #[ts(type = "string")]
    pub opened_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Serialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PreviewTicket {
    pub ticket: String,
    /// Absolute URL that redeems the ticket and lands on the preview.
    pub auth_url: String,
    pub expires_in_secs: u32,
}

fn info(state: &AppState, preview: &Preview) -> PreviewInfo {
    PreviewInfo {
        id: preview.id.clone(),
        port: preview.port,
        url: state.preview.url_for(&preview.id),
        opened_at: preview.opened_at,
    }
}

async fn require_owner(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<Uuid, StatusCode> {
    let owner = crate::authz::session_owner(session_id, &state.pool).await.map_err(|e| {
        tracing::error!("db error (preview owner): {e}");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    match owner {
        Some(owner) if owner == ctx.user_id => Ok(owner),
        Some(_) => Err(StatusCode::FORBIDDEN),
        None => Err(StatusCode::NOT_FOUND),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<PreviewInfo>>, StatusCode> {
    require_owner(&state, &ctx, &session_id).await?;
    Ok(Json(state.preview.list(&session_id).iter().map(|p| info(&state, p)).collect()))
}

pub async fn ticket(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((session_id, preview_id)): Path<(String, String)>,
) -> Result<Json<PreviewTicket>, StatusCode> {
    let owner = require_owner(&state, &ctx, &session_id).await?;
    let Some(preview) = state.preview.get(&preview_id) else {
        return Err(StatusCode::NOT_FOUND);
    };
    if preview.session_id != session_id || preview.user_id != owner {
        return Err(StatusCode::NOT_FOUND);
    }
    let ticket = state.preview.tickets().mint_ticket(&preview.id, owner);
    let auth_url = format!("{}/__cctui/auth?ticket={ticket}", state.preview.url_for(&preview.id));
    Ok(Json(PreviewTicket {
        ticket,
        auth_url,
        expires_in_secs: u32::try_from(super::ticket::TICKET_TTL_SECS).unwrap_or(60),
    }))
}
