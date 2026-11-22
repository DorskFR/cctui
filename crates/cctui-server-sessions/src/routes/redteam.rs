use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::redteam::{
    REDTEAM_MAX_REPORT_BYTES, RedteamOverview, RedteamRunAccepted, RedteamRunReport,
};

use crate::auth::{AuthContext, Scope};
use crate::context::SessionsContext;
use crate::error::AppError;
use crate::redteam_runs;

fn parse(body: &[u8]) -> Result<RedteamRunReport, AppError> {
    if body.len() > REDTEAM_MAX_REPORT_BYTES {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("a red-team report is at most {REDTEAM_MAX_REPORT_BYTES} bytes"),
        ));
    }
    let report: RedteamRunReport = serde_json::from_slice(body)
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, format!("invalid report: {e}")))?;
    report.validate().map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    Ok(report)
}

pub async fn record<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    body: Bytes,
) -> Result<(StatusCode, Json<RedteamRunAccepted>), AppError> {
    ctx.requires(Scope::Dispatch).map_err(|s| AppError::new(s, "dispatch scope required"))?;
    let report = parse(&body)?;
    let id = redteam_runs::insert(state.pool(), &report, ctx.user_id).await?;
    tracing::info!(
        user = %ctx.user_id,
        tier = report.tier.as_str(),
        lane = %report.lane,
        version = %report.version,
        verdict = report.verdict.as_str(),
        "red-team run recorded"
    );
    Ok((StatusCode::CREATED, Json(RedteamRunAccepted { id })))
}

pub async fn overview<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<RedteamOverview>, AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))?;
    Ok(Json(redteam_runs::overview(state.pool(), env!("CARGO_PKG_VERSION")).await?))
}
