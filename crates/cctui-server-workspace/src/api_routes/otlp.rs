//! Browser trace export, forwarded to this server's own OTLP collector.

use std::sync::LazyLock;
use std::time::Instant;

use axum::Extension;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::routing::post;
use serde_json::Value;

use crate::auth::AuthContext;
use crate::authz::Authz::Human;
use crate::authz::{Authn, Routes};
use crate::routes::peer::Limiter;
use crate::state::AppState;

const BODY_LIMIT: usize = 1024 * 1024;
/// The browser batches spans every few seconds; this leaves room for many tabs.
const EXPORTS_PER_USER_PER_MIN: usize = 120;
/// The webui's default `service.name`; a configured override must keep this prefix.
const WEBUI_SERVICE: &str = "cctui-webui";

static BY_USER: LazyLock<Limiter> = LazyLock::new(Limiter::default);

pub(super) fn register(r: Routes) -> Routes {
    r.add(
        &[Method::POST],
        "/otlp/v1/traces",
        "Forward the webui's OTLP trace export to this server's collector.",
        post(forward).layer(DefaultBodyLimit::max(BODY_LIMIT)),
        Authn::Bearer,
        Human,
    )
}

async fn forward(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    let Some(url) = traces_url(cctui_telemetry::enabled(), |key| std::env::var(key).ok()) else {
        return StatusCode::NOT_FOUND;
    };
    if let Err(status) = check_payload(&headers, &body) {
        return status;
    }
    if !BY_USER.admit(&ctx.user_id.to_string(), EXPORTS_PER_USER_PER_MIN, Instant::now()) {
        return StatusCode::TOO_MANY_REQUESTS;
    }
    let mut request =
        state.http_client.post(url).header(header::CONTENT_TYPE, "application/json").body(body);
    if let Some(value) = headers.get(header::CONTENT_ENCODING) {
        request = request.header(header::CONTENT_ENCODING, value.clone());
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

/// Only uncompressed OTLP/JSON whose every resource is the webui is relayed:
/// the body must be inspectable, and a key holder must not be able to inject
/// spans under another service's name.
fn check_payload(headers: &HeaderMap, body: &[u8]) -> Result<(), StatusCode> {
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(';').next().is_some_and(|m| m.trim() == "application/json"));
    let identity = headers
        .get(header::CONTENT_ENCODING)
        .is_none_or(|v| v.as_bytes().eq_ignore_ascii_case(b"identity"));
    if !json || !identity {
        return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }
    let payload: Value = serde_json::from_slice(body).map_err(|_| StatusCode::BAD_REQUEST)?;
    let resources = payload.get("resourceSpans").and_then(Value::as_array);
    let all_webui = resources.is_some_and(|r| !r.is_empty() && r.iter().all(is_webui_resource));
    if all_webui { Ok(()) } else { Err(StatusCode::BAD_REQUEST) }
}

fn is_webui_resource(resource_spans: &Value) -> bool {
    resource_spans
        .pointer("/resource/attributes")
        .and_then(Value::as_array)
        .and_then(|attrs| {
            attrs.iter().find(|a| a.get("key").and_then(Value::as_str) == Some("service.name"))
        })
        .and_then(|a| a.pointer("/value/stringValue"))
        .and_then(Value::as_str)
        .is_some_and(|name| {
            name == WEBUI_SERVICE
                || name.strip_prefix(WEBUI_SERVICE).is_some_and(|rest| rest.starts_with('-'))
        })
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
    use axum::middleware::{self, Next};
    use http::Request as HttpRequest;
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

    #[tokio::test]
    async fn machine_key_is_403() {
        let mut machine = user();
        machine.machine_id = Some(Uuid::new_v4());
        assert_eq!(post_status(app(Some(machine)), b"{}".to_vec()).await, StatusCode::FORBIDDEN);
    }

    fn json_headers() -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        headers
    }

    fn export(services: &[&str]) -> Vec<u8> {
        let resource_spans: Vec<serde_json::Value> = services
            .iter()
            .map(|name| {
                serde_json::json!({
                    "resource": { "attributes": [
                        { "key": "service.version", "value": { "stringValue": "1" } },
                        { "key": "service.name", "value": { "stringValue": name } }
                    ] },
                    "scopeSpans": []
                })
            })
            .collect();
        serde_json::to_vec(&serde_json::json!({ "resourceSpans": resource_spans })).unwrap()
    }

    #[test]
    fn webui_service_payloads_are_relayed() {
        assert_eq!(check_payload(&json_headers(), &export(&["cctui-webui"])), Ok(()));
        let both = export(&["cctui-webui-dev", "cctui-webui"]);
        assert_eq!(check_payload(&json_headers(), &both), Ok(()));
        let mut charset = json_headers();
        charset.insert(header::CONTENT_TYPE, "application/json; charset=utf-8".parse().unwrap());
        assert_eq!(check_payload(&charset, &export(&["cctui-webui"])), Ok(()));
    }

    #[test]
    fn foreign_or_missing_service_names_are_dropped() {
        let bad = Err(StatusCode::BAD_REQUEST);
        assert_eq!(check_payload(&json_headers(), &export(&["cctui-server"])), bad);
        assert_eq!(check_payload(&json_headers(), &export(&["cctui-webui", "cctui-daemon"])), bad);
        assert_eq!(check_payload(&json_headers(), &export(&["cctui-webuix"])), bad);
        assert_eq!(check_payload(&json_headers(), &export(&[])), bad);
        assert_eq!(check_payload(&json_headers(), br#"{"resourceSpans":[{"resource":{}}]}"#), bad);
        assert_eq!(check_payload(&json_headers(), b"not json"), bad);
    }

    #[test]
    fn uninspectable_bodies_are_415() {
        let mut proto = HeaderMap::new();
        proto.insert(header::CONTENT_TYPE, "application/x-protobuf".parse().unwrap());
        let unsupported = Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert_eq!(check_payload(&proto, &export(&["cctui-webui"])), unsupported);
        assert_eq!(check_payload(&HeaderMap::new(), &export(&["cctui-webui"])), unsupported);
        let mut gzip = json_headers();
        gzip.insert(header::CONTENT_ENCODING, "gzip".parse().unwrap());
        assert_eq!(check_payload(&gzip, &export(&["cctui-webui"])), unsupported);
    }

    #[test]
    fn exports_are_throttled_per_user() {
        let limiter = Limiter::default();
        let now = Instant::now();
        for _ in 0..EXPORTS_PER_USER_PER_MIN {
            assert!(limiter.admit("u1", EXPORTS_PER_USER_PER_MIN, now));
        }
        assert!(!limiter.admit("u1", EXPORTS_PER_USER_PER_MIN, now));
        assert!(limiter.admit("u2", EXPORTS_PER_USER_PER_MIN, now));
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
