//! Session API: list previews, mint access tickets. Owner only.

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use cctui_proto::api::ApiError;
use uuid::Uuid;

use super::Preview;
use crate::auth::AuthContext;
use crate::state::AppState;

pub use cctui_proto::api::previews::{PreviewInfo, PreviewTicket};

fn info(state: &AppState, preview: &Preview) -> PreviewInfo {
    PreviewInfo {
        id: preview.id.clone(),
        port: preview.port,
        url: state.preview.url_for(&preview.id),
        opened_at: preview.opened_at,
    }
}

type ApiErr = (StatusCode, Json<ApiError>);

fn err(status: StatusCode, msg: impl Into<String>) -> ApiErr {
    (status, Json(ApiError { error: msg.into() }))
}

/// A caller that would otherwise poll an always-empty preview list needs to be
/// told the feature is off, not left waiting.
fn require_enabled(state: &AppState) -> Result<(), ApiErr> {
    if state.preview.enabled() {
        return Ok(());
    }
    Err(err(StatusCode::SERVICE_UNAVAILABLE, cctui_proto::ws::PREVIEWS_DISABLED))
}

async fn require_owner(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<Uuid, ApiErr> {
    let owner = crate::authz::session_owner(session_id, &state.pool).await.map_err(|e| {
        tracing::error!("db error (preview owner): {e}");
        err(StatusCode::INTERNAL_SERVER_ERROR, "could not resolve the session owner")
    })?;
    match owner {
        Some(owner) if owner == ctx.user_id => Ok(owner),
        Some(_) => Err(err(StatusCode::FORBIDDEN, "not your session")),
        None => Err(err(StatusCode::NOT_FOUND, "no such session")),
    }
}

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<PreviewInfo>>, ApiErr> {
    require_enabled(&state)?;
    require_owner(&state, &ctx, &session_id).await?;
    let previews = state.preview.list(&state.pool, &session_id).await;
    Ok(Json(previews.iter().map(|p| info(&state, p)).collect()))
}

pub async fn ticket(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((session_id, preview_id)): Path<(String, String)>,
) -> Result<Json<PreviewTicket>, ApiErr> {
    require_enabled(&state)?;
    let owner = require_owner(&state, &ctx, &session_id).await?;
    let Some(preview) = state.preview.get(&state.pool, &preview_id).await else {
        return Err(err(StatusCode::NOT_FOUND, "no such preview"));
    };
    if preview.session_id != session_id || preview.user_id != owner {
        return Err(err(StatusCode::NOT_FOUND, "no such preview"));
    }
    let ticket = state.preview.tickets().mint_ticket(&preview.id, owner);
    let auth_url = format!("{}/__cctui/auth?ticket={ticket}", state.preview.url_for(&preview.id));
    Ok(Json(PreviewTicket {
        ticket,
        auth_url,
        expires_in_secs: u32::try_from(super::ticket::TICKET_TTL_SECS).unwrap_or(60),
    }))
}
