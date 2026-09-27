//! Cross-site request forgery gate for cookie-authenticated API calls.
//!
//! Preview hosts are same-site with cctui, so `SameSite=Lax` alone no longer
//! keeps a script running in a previewed app from posting to the API with
//! the browser's `cctui_auth` cookie. Unsafe methods carried by that cookie
//! must therefore come from an allowed origin. Bearer requests never carry
//! ambient credentials and pass untouched.

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

fn bearer_present(headers: &HeaderMap) -> bool {
    headers
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("Bearer "))
}

fn origin_of_referer(referer: &str) -> Option<String> {
    let (scheme, rest) = referer.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}"))
}

/// The request's claimed origin: `Origin`, else the origin of `Referer`.
#[must_use]
pub fn request_origin(headers: &HeaderMap) -> Option<String> {
    if let Some(origin) = headers.get(http::header::ORIGIN).and_then(|v| v.to_str().ok()) {
        return Some(origin.trim().to_owned());
    }
    headers.get(http::header::REFERER).and_then(|v| v.to_str().ok()).and_then(origin_of_referer)
}

/// Whether the request may proceed under the CSRF policy.
#[must_use]
pub fn allows(allowed_origins: &[String], method: &Method, headers: &HeaderMap) -> bool {
    let unsafe_method =
        matches!(*method, Method::POST | Method::PUT | Method::PATCH | Method::DELETE);
    if !unsafe_method || bearer_present(headers) {
        return true;
    }
    if crate::auth::token_from_cookies(headers).is_none() {
        return true;
    }
    request_origin(headers).is_some_and(|origin| {
        allowed_origins.iter().any(|o| o.eq_ignore_ascii_case(origin.trim_end_matches('/')))
    })
}

pub async fn middleware(
    State(allowed_origins): State<Arc<Vec<String>>>,
    request: Request,
    next: Next,
) -> Response {
    if allows(&allowed_origins, request.method(), request.headers()) {
        return next.run(request).await;
    }
    tracing::warn!(
        method = %request.method(),
        path = %request.uri().path(),
        origin = request_origin(request.headers()).as_deref().unwrap_or("<none>"),
        "rejected cookie-authenticated request from a foreign origin",
    );
    (StatusCode::FORBIDDEN, "cross-site request rejected").into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::{get, post};
    use tower::ServiceExt;

    fn app() -> Router {
        let origins = Arc::new(vec!["https://cctui.example".to_owned()]);
        Router::new()
            .route("/x", get(|| async { "get" }).post(|| async { "post" }))
            .route("/y", post(|| async { "post" }))
            .layer(axum::middleware::from_fn_with_state(origins, middleware))
    }

    async fn status(method: &str, headers: &[(&str, &str)]) -> StatusCode {
        let mut req = http::Request::builder().method(method).uri("/x");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        app().oneshot(req.body(Body::empty()).unwrap()).await.unwrap().status()
    }

    #[tokio::test]
    async fn cookie_posts_need_an_allowed_origin() {
        let cookie = ("cookie", "cctui_auth=tok");
        assert_eq!(
            status("POST", &[cookie, ("origin", "https://evil.example")]).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(status("POST", &[cookie]).await, StatusCode::FORBIDDEN);
        assert_eq!(
            status("POST", &[cookie, ("referer", "https://cctui-pv-abc.example/app")]).await,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            status("POST", &[cookie, ("origin", "https://cctui.example")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status("POST", &[cookie, ("origin", "HTTPS://CCTUI.EXAMPLE/")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status("POST", &[cookie, ("referer", "https://cctui.example/sessions/1?x=1")]).await,
            StatusCode::OK
        );
        assert_eq!(
            status("DELETE", &[cookie, ("origin", "https://evil.example")]).await,
            StatusCode::FORBIDDEN
        );
    }

    #[tokio::test]
    async fn bearer_and_safe_methods_pass() {
        assert_eq!(
            status("POST", &[("authorization", "Bearer t"), ("origin", "https://evil.example")])
                .await,
            StatusCode::OK
        );
        assert_eq!(
            status(
                "POST",
                &[
                    ("authorization", "Bearer t"),
                    ("cookie", "cctui_auth=tok"),
                    ("origin", "https://evil.example")
                ]
            )
            .await,
            StatusCode::OK
        );
        assert_eq!(
            status("GET", &[("cookie", "cctui_auth=tok"), ("origin", "https://evil.example")])
                .await,
            StatusCode::OK
        );
        assert_eq!(
            status("POST", &[("origin", "https://evil.example")]).await,
            StatusCode::OK,
            "no credential at all"
        );
        assert_eq!(
            status("POST", &[("cookie", "other=1"), ("origin", "https://evil.example")]).await,
            StatusCode::OK
        );
    }

    #[test]
    fn referer_origin_extraction() {
        assert_eq!(
            origin_of_referer("https://a.b:8443/p?q#f").as_deref(),
            Some("https://a.b:8443")
        );
        assert_eq!(origin_of_referer("nonsense"), None);
        assert_eq!(origin_of_referer("https:///x"), None);
    }
}
