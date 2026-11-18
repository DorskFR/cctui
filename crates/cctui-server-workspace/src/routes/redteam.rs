use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::redteam::{
    REDTEAM_MAX_REPORT_BYTES, RedteamOverview, RedteamRunAccepted, RedteamRunReport,
};

use crate::auth::{AuthContext, Scope};
use crate::error::AppError;
use crate::redteam_runs;
use crate::state::AppState;

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

pub async fn record(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    body: Bytes,
) -> Result<(StatusCode, Json<RedteamRunAccepted>), AppError> {
    ctx.requires(Scope::Dispatch).map_err(|s| AppError::new(s, "dispatch scope required"))?;
    let report = parse(&body)?;
    let id = redteam_runs::insert(&state.pool, &report, ctx.user_id).await?;
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

pub async fn overview(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<RedteamOverview>, AppError> {
    ctx.requires(Scope::Admin).map_err(|s| AppError::new(s, "admin token required"))?;
    Ok(Json(redteam_runs::overview(&state.pool, env!("CARGO_PKG_VERSION")).await?))
}

#[cfg(test)]
mod tests {
    use axum::Extension;
    use axum::body::{Body, Bytes};
    use axum::extract::State;
    use axum::http::{Request, StatusCode};
    use cctui_proto::api::redteam::REDTEAM_MAX_REPORT_BYTES;
    use cctui_proto::features::FeatureSet;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::{overview, record};
    use crate::auth::{AuthContext, Scope};
    use crate::state::AppState;

    fn ctx(scopes: &[Scope]) -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::nil(),
            machine_id: None,
            scopes: scopes.iter().copied().collect(),
        }
    }

    fn state() -> AppState {
        AppState::for_test(sqlx::PgPool::connect_lazy("postgres://unused@localhost/none").unwrap())
    }

    fn fixture() -> Bytes {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../redteam/ci/run-report.fixture.json");
        Bytes::from(std::fs::read(path).unwrap())
    }

    async fn post(scopes: &[Scope], body: Bytes) -> StatusCode {
        record(State(state()), Extension(ctx(scopes)), body).await.unwrap_err().status()
    }

    #[tokio::test]
    async fn non_admins_cannot_read_red_team_results() {
        let non_admin: [&[Scope]; 3] =
            [&[], &[Scope::Read], &[Scope::Read, Scope::Dispatch, Scope::Enroll]];
        for scopes in non_admin {
            let denied = overview(State(state()), Extension(ctx(scopes))).await;
            assert_eq!(denied.unwrap_err().status(), StatusCode::FORBIDDEN);
        }
    }

    #[tokio::test]
    async fn posting_needs_the_dispatch_scope() {
        assert_eq!(post(&[Scope::Read], fixture()).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn an_oversize_report_is_refused_before_it_is_parsed() {
        let mut body = fixture().to_vec();
        body.resize(REDTEAM_MAX_REPORT_BYTES + 1, b' ');
        let status = post(&[Scope::Read, Scope::Dispatch], Bytes::from(body)).await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn a_malformed_or_out_of_bound_report_is_a_bad_request() {
        let poster = [Scope::Read, Scope::Dispatch];
        assert_eq!(post(&poster, Bytes::from_static(b"{")).await, StatusCode::BAD_REQUEST);
        let mut report: serde_json::Value = serde_json::from_slice(&fixture()).unwrap();
        report["version"] = "../../x".into();
        let body = Bytes::from(serde_json::to_vec(&report).unwrap());
        assert_eq!(post(&poster, body).await, StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn both_routes_answer_401_without_a_token() {
        let router = crate::build_api_routes_with(FeatureSet::all()).into_parts().0;
        let app = router.with_state(state());
        for (method, uri) in [("POST", "/redteam/runs"), ("GET", "/admin/redteam")] {
            let request = Request::builder()
                .method(method)
                .uri(uri)
                .header("content-type", "application/json")
                .body(Body::from(fixture()))
                .unwrap();
            let status = app.clone().oneshot(request).await.unwrap().status();
            assert_eq!(status, StatusCode::UNAUTHORIZED, "{method} {uri}");
        }
    }
}
