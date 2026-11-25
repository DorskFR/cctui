//! `GET /api/v1/provider-status` — what the upstream families report.

use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::Response;

use crate::context::AccountsHost;

/// Every polled family's last reading, served from
/// [`crate::provider_status::ProviderStatusCache`].
///
/// Reads no network and cannot
/// fail: a family with no reading answers `unknown`. `ETag`d because the answer
/// changes at most once a minute and clients poll it.
#[allow(clippy::unused_async)]
pub async fn provider_status<S: AccountsHost>(
    State(state): State<S>,
    headers: HeaderMap,
) -> Response {
    cctui_server_kernel::http_cache::json_with_etag(&headers, &state.provider_status().all())
}
