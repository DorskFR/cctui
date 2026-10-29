//! Daemon distribution shared with the execution host.
use crate::state::AppState;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
pub use cctui_server_core::targets::manifest::repo;
use cctui_server_core::targets::manifest::{self, DistributionContext, ManifestQuery};
impl DistributionContext for AppState {
    fn distribution_url(&self) -> &str {
        &self.config.external_url
    }
    fn github_token(&self) -> Option<&str> {
        self.config.github_token.as_deref()
    }
}
pub async fn daemon_manifest(
    state: State<AppState>,
    query: Query<ManifestQuery>,
    headers: HeaderMap,
) -> Response {
    manifest::daemon_manifest(state, query, headers).await
}
pub async fn download_daemon_binary(
    state: State<AppState>,
    target: Path<String>,
) -> Result<Response, (StatusCode, String)> {
    manifest::download_daemon_binary(state, target).await
}
