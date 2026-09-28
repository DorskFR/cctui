use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use serde::Serialize;

use crate::error::AppError;
use crate::routes::instance;
use crate::state::AppState;
use crate::update_check;

const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("CCTUI_GIT_HASH");
const REPO_URL: &str = "https://github.com/DorskFR/cctui";

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct VersionInfo {
    pub version: &'static str,
    pub git_hash: &'static str,
    pub repo_url: &'static str,
    pub commit_url: String,
    /// Newest upstream release, present **only** when strictly newer than
    /// `version` (see `update_check`). `null` when up to date, when the probe
    /// is disabled, or before its first answer.
    pub latest_version: Option<String>,
    /// Release page for `latest_version`.
    pub latest_url: Option<String>,
    /// Admin-set deployment label (`PUT /admin/instance`); `null` by default,
    /// and always `null` for an unauthenticated caller.
    pub instance_name: Option<String>,
    /// Whether an admin can launch a self-update from here: a self-update
    /// machine is configured (settings or env). Everyone sees the flag, only
    /// admins get the button; the machine itself stays admin-only.
    pub self_update_ready: bool,
    /// Whether that machine has a deterministic update hook
    /// (`CCTUI_UPDATE_COMMAND`), so the update runs the operator's own command
    /// instead of a YOLO agent. Drives what the update modal promises.
    pub self_update_hook: bool,
}

#[derive(Serialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChangelogResponse {
    pub version: &'static str,
    /// Releases published since `version`, newest first (capped server-side).
    /// Empty when up to date or before the probe's first answer.
    pub releases: Vec<update_check::ReleaseNote>,
}

/// Release notes of every upstream release newer than this build, as the
/// background probe last saw them. No network call: the modal opens instantly.
pub async fn changelog(State(state): State<AppState>) -> Json<ChangelogResponse> {
    Json(ChangelogResponse { version: VERSION, releases: state.update_check.notes().await })
}

/// Public: `cctui update` and the TUI self-update have no token. An anonymous
/// caller gets build + update-manifest fields only; the deployment label and
/// self-update readiness require a valid credential.
pub async fn version(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Json<VersionInfo> {
    match crate::auth::bearer_or_cookie(&headers) {
        Some(token) if state.auth_config.validate(&token).await.is_some() => {
            Json(info(&state).await)
        }
        _ => Json(manifest_info(&state).await),
    }
}

/// Probe upstream now instead of waiting out the 6h interval, then answer with
/// the same payload as `GET /version` so the caller can swap its cached copy.
///
/// Clicks inside [`update_check::MANUAL_COOLDOWN`] reuse the last answer rather
/// than querying GitHub again; a probe that fails surfaces as `502` so the
/// webui can say so instead of silently showing a stale "up to date".
pub async fn refresh_version(State(state): State<AppState>) -> Result<Json<VersionInfo>, AppError> {
    if !update_check::enabled_from_env() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "update check is disabled on this server (CCTUI_UPDATE_CHECK=0)",
        ));
    }
    state
        .update_check
        .refresh(&state.http_client)
        .await
        .map_err(|e| AppError::new(StatusCode::BAD_GATEWAY, format!("update check failed: {e}")))?;
    Ok(Json(info(&state).await))
}

/// This build plus the upstream release manifest — all of it already public (the
/// repo is). The whole payload an unauthenticated caller may see: no deployment
/// label, no self-update state, no database read.
async fn manifest_info(state: &AppState) -> VersionInfo {
    let commit_url = if GIT_HASH == "unknown" {
        REPO_URL.to_string()
    } else {
        format!("{REPO_URL}/commit/{GIT_HASH}")
    };
    let latest = state.update_check.newer().await;
    VersionInfo {
        version: VERSION,
        git_hash: GIT_HASH,
        repo_url: REPO_URL,
        commit_url,
        latest_version: latest.as_ref().map(|l| l.version.clone()),
        latest_url: latest.map(|l| l.url),
        instance_name: None,
        self_update_ready: false,
        self_update_hook: false,
    }
}

async fn info(state: &AppState) -> VersionInfo {
    let instance_name = instance::read_name(&state.pool).await;
    let self_update_target = instance::read_self_update_target(&state.pool).await.target;
    let self_update_ready = self_update_target.is_some();
    let self_update_hook = match self_update_target
        .and_then(|t| uuid::Uuid::parse_str(&t.machine_id).ok())
    {
        Some(machine) => crate::routes::update_hook::machine_has_hook(&state.pool, machine).await,
        None => false,
    };
    VersionInfo { instance_name, self_update_ready, self_update_hook, ..manifest_info(state).await }
}

#[cfg(test)]
mod tests {
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    use crate::state::AppState;

    fn state() -> AppState {
        AppState::for_test(sqlx::PgPool::connect_lazy("postgres://invalid").unwrap())
    }

    fn routers() -> (Router, Router) {
        let (authed, public, _) = crate::build_api_routes().into_parts();
        (authed.with_state(state()), public.with_state(state()))
    }

    async fn get(app: Router, uri: &str) -> (StatusCode, String) {
        let res =
            app.oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap()).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 64 * 1024).await.unwrap();
        (status, String::from_utf8_lossy(&bytes).to_string())
    }

    /// No credential, no 401. The invalid pool also proves the anonymous payload
    /// reads no database.
    #[tokio::test]
    async fn version_answers_without_a_token() {
        let (_, public) = routers();
        let (status, body) = get(public, "/api/v1/version").await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        assert!(body.contains(env!("CARGO_PKG_VERSION")), "body: {body}");
    }

    /// An anonymous caller sees build + upstream-release fields only.
    #[tokio::test]
    async fn anonymous_version_exposes_no_deployment_state() {
        let (_, public) = routers();
        let (_, body) = get(public, "/api/v1/version").await;
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v["instance_name"].is_null(), "instance name leaked: {body}");
        assert_eq!(v["self_update_ready"], serde_json::json!(false));
        assert_eq!(v["self_update_hook"], serde_json::json!(false));

        // Every field of the response is one of these; a new one must be
        // classified as public or gated rather than shipping by default.
        let public_fields = [
            "version",
            "git_hash",
            "repo_url",
            "commit_url",
            "latest_version",
            "latest_url",
            "instance_name",
            "self_update_ready",
            "self_update_hook",
        ];
        for key in v.as_object().unwrap().keys() {
            assert!(public_fields.contains(&key.as_str()), "unclassified field {key} on /version");
        }
    }

    /// A bad credential is anonymous, not a 401: the public route never rejects.
    #[tokio::test]
    async fn version_ignores_an_invalid_token() {
        let (_, public) = routers();
        let res = public
            .oneshot(
                Request::builder()
                    .uri("/api/v1/version")
                    .header("authorization", "Bearer cctui_u_nope")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    /// It must not also sit on the authenticated router, where the auth layer
    /// would 401 it again.
    #[tokio::test]
    async fn version_is_not_on_the_authenticated_router() {
        let (authed, _) = routers();
        let (status, _) = get(authed, "/version").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }
}
