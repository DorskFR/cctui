//! Browser trace export, forwarded to this server's own OTLP collector.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::routing::post;

use crate::authz::Authz::Authenticated;
use crate::authz::{Authn, Routes};
use crate::state::AppState;

const BODY_LIMIT: usize = 1024 * 1024;
const FORWARDED: [header::HeaderName; 2] = [header::CONTENT_TYPE, header::CONTENT_ENCODING];

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/otlp/v1/traces",
        "Forward the webui's OTLP trace export to this server's collector.",
        post(forward).layer(DefaultBodyLimit::max(BODY_LIMIT)),
        Authn::Bearer,
        Authenticated,
    )
}

async fn forward(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> StatusCode {
    let Some(url) = traces_url(cctui_telemetry::enabled(), |key| std::env::var(key).ok()) else {
        return StatusCode::NOT_FOUND;
    };
    let mut request = state.http_client.post(url).body(body);
    for name in FORWARDED {
        if let Some(value) = headers.get(&name) {
            request = request.header(name, value.clone());
        }
    }
    match request.send().await {
        Ok(response) if response.status().is_success() => StatusCode::OK,
        Ok(response) => {
            tracing::warn!(status = %response.status(), "OTLP collector rejected browser spans");
            StatusCode::BAD_GATEWAY
        }
        Err(err) => {
            tracing::warn!(%err, "OTLP collector unreachable for browser spans");
            StatusCode::BAD_GATEWAY
        }
    }
}

fn traces_url(enabled: bool, var: impl Fn(&str) -> Option<String>) -> Option<String> {
    if !enabled {
        return None;
    }
    let set = |key: &str| var(key).map(|v| v.trim().to_owned()).filter(|v| !v.is_empty());
    set("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").or_else(|| {
        set("OTEL_EXPORTER_OTLP_ENDPOINT")
            .map(|base| format!("{}/v1/traces", base.trim_end_matches('/')))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use axum::Router;
    use axum::body::Body;
    use axum::extract::Request;
    use http::Request as HttpRequest;
    use axum::middleware::{self, Next};
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;
    use crate::auth::{AuthContext, Scope};

    fn app(ctx: Option<AuthContext>) -> Router {
        let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
        let routes = register(Routes::new()).into_parts().0;
        let routes = match ctx {
            Some(ctx) => routes.layer(middleware::from_fn(move |mut req: Request, next: Next| {
                let ctx = ctx.clone();
                async move {
                    req.extensions_mut().insert(ctx);
                    next.run(req).await
                }
            })),
            None => routes,
        };
        routes.with_state(AppState::for_test(pool))
    }

    fn user() -> AuthContext {
        AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: std::iter::once(Scope::Read).collect(),
        }
    }

    async fn post_status(app: Router, body: Vec<u8>) -> StatusCode {
        let request = HttpRequest::builder()
            .method(Method::POST)
            .uri("/otlp/v1/traces")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body))
            .unwrap();
        app.oneshot(request).await.unwrap().status()
    }

    #[tokio::test]
    async fn unauthenticated_is_401() {
        assert_eq!(post_status(app(None), b"{}".to_vec()).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn disabled_export_is_404() {
        assert!(!cctui_telemetry::enabled());
        assert_eq!(post_status(app(Some(user())), b"{}".to_vec()).await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn body_over_one_mebibyte_is_413() {
        let body = vec![b'x'; BODY_LIMIT + 1];
        assert_eq!(post_status(app(Some(user())), body).await, StatusCode::PAYLOAD_TOO_LARGE);
    }

    fn url(env: &[(&str, &str)]) -> Option<String> {
        let env: HashMap<String, String> =
            env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        traces_url(true, |key| env.get(key).cloned())
    }

    #[test]
    fn base_endpoint_gains_the_traces_path() {
        assert_eq!(
            url(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://hoshizora:4318")]).as_deref(),
            Some("http://hoshizora:4318/v1/traces")
        );
        assert_eq!(
            url(&[("OTEL_EXPORTER_OTLP_ENDPOINT", "http://hoshizora:4318/")]).as_deref(),
            Some("http://hoshizora:4318/v1/traces")
        );
    }

    #[test]
    fn traces_endpoint_is_used_verbatim_and_wins() {
        let env = [
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://base:4318"),
            ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "http://traces:4318/custom"),
        ];
        assert_eq!(url(&env).as_deref(), Some("http://traces:4318/custom"));
    }

    #[test]
    fn blank_or_missing_endpoints_resolve_to_none() {
        assert_eq!(url(&[]), None);
        assert_eq!(url(&[("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", " ")]), None);
        let env = [
            ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", ""),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", "http://b"),
        ];
        assert_eq!(url(&env).as_deref(), Some("http://b/v1/traces"));
    }

    #[test]
    fn disabled_exporter_resolves_to_none() {
        assert_eq!(traces_url(false, |_| Some("http://hoshizora:4318".to_owned())), None);
    }
}
