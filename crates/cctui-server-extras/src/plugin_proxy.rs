//! Authenticated reverse proxy to a plugin's own backend.
//!
//! `GET|POST|PUT|PATCH|DELETE /api/v1/plugins/{id}/backend/{*path}` authenticates with the normal
//! cctui cookie or bearer, then forwards to the upstream an admin configured,
//! carrying a signed identity instead of any cctui credential. The plugin
//! backend therefore needs no cctui database grant and no browser token: it
//! trusts `X-Cctui-Sig` under a secret only it and this server know.
//!
//! The signature covers method, path, timestamp and user id — see
//! `docs/plugins.md` for the canonical string, which the ghreview verifier and
//! the shared vectors in `docs/plugin-proxy-signature-vectors.json` must agree
//! with byte for byte.

use axum::Extension;
use axum::body::Body;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::http::header::{HeaderMap, HeaderName, HeaderValue};
use axum::response::{IntoResponse, Response};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::LazyLock;

use crate::context::ExtrasContext;
use cctui_server_kernel::auth::AuthContext;

pub const USER_ID_HEADER: &str = "x-cctui-user-id";
pub const USER_NAME_HEADER: &str = "x-cctui-user-name";
pub const PLUGIN_HEADER: &str = "x-cctui-plugin";
pub const TS_HEADER: &str = "x-cctui-ts";
pub const SIG_HEADER: &str = "x-cctui-sig";

/// Headers that are per-connection, or that would let a caller impersonate the
/// proxy's own identity assertion.
fn strip(name: &str) -> bool {
    matches!(
        name,
        "host"
            | "authorization"
            | "connection"
            | "keep-alive"
            | "proxy-authenticate"
            | "proxy-authorization"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
            | "content-length"
    ) || name.starts_with("x-cctui-")
}

fn response_strip(name: &str) -> bool {
    matches!(
        name,
        "connection"
            | "keep-alive"
            | "transfer-encoding"
            | "trailer"
            | "upgrade"
            | "content-length"
    )
}

/// The string the proxy signs. Keep this the single definition; the ghreview
/// verifier reproduces it and the shared vectors pin it.
#[must_use]
pub fn canonical(method: &str, path: &str, ts: i64, user_id: &str) -> String {
    format!("{method}\n{path}\n{ts}\n{user_id}")
}

#[must_use]
pub fn sign(secret: &str, method: &str, path: &str, ts: i64, user_id: &str) -> String {
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("hmac accepts any key length");
    mac.update(canonical(method, path, ts, user_id).as_bytes());
    hex::encode(mac.finalize().into_bytes())
}

/// The path the signature covers: always exactly one leading slash and no
/// query string, so the two sides cannot disagree over `//a` or `?x=1`.
#[must_use]
pub fn signed_path(raw: &str) -> String {
    let without_query = raw.split(['?', '#']).next().unwrap_or("");
    format!("/{}", without_query.trim_start_matches('/'))
}

/// A client dedicated to plugin upstreams: no redirects (a redirect could point
/// the proxy at an address the admin never configured) and no response timeout,
/// so an `text/event-stream` upstream can stay open indefinitely.
fn client() -> &'static reqwest::Client {
    static CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    });
    &CLIENT
}

fn text(status: StatusCode, body: &str) -> Response {
    (status, body.to_owned()).into_response()
}

fn forwarded_headers(
    src: &HeaderMap,
    plugin_id: &str,
    user_id: &str,
    user_name: &str,
    ts: i64,
    signature: &str,
) -> HeaderMap {
    let mut out = HeaderMap::new();
    for (name, value) in src {
        if strip(name.as_str()) {
            continue;
        }
        if name == axum::http::header::COOKIE {
            if let Some(kept) = value.to_str().ok().and_then(crate::preview::app_cookies)
                && let Ok(kept) = HeaderValue::from_str(&kept)
            {
                out.append(name.clone(), kept);
            }
            continue;
        }
        out.append(name.clone(), value.clone());
    }
    let mut set = |name: &str, value: &str| {
        if let (Ok(name), Ok(value)) =
            (HeaderName::from_bytes(name.as_bytes()), HeaderValue::from_str(value))
        {
            out.insert(name, value);
        }
    };
    set(USER_ID_HEADER, user_id);
    set(USER_NAME_HEADER, user_name);
    set(PLUGIN_HEADER, plugin_id);
    set(TS_HEADER, &ts.to_string());
    set(SIG_HEADER, signature);
    out
}

/// Join the admin-configured base with the caller's sub-path and query.
#[must_use]
pub fn upstream_url(base: &str, path: &str, query: Option<&str>) -> String {
    let base = base.trim_end_matches('/');
    let path = path.trim_start_matches('/');
    let mut url = format!("{base}/{path}");
    if let Some(query) = query.filter(|q| !q.is_empty()) {
        url.push('?');
        url.push_str(query);
    }
    url
}

pub async fn backend<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, path)): Path<(String, String)>,
    request: Request,
) -> Response {
    if ctx.requires(cctui_server_kernel::auth::Scope::Read).is_err() {
        return text(StatusCode::FORBIDDEN, "this credential may not reach plugin backends");
    }
    if state.plugins().get(&id).is_none() {
        crate::plugin_store::sync_or_warn(state.pool(), state.plugins(), &state).await;
    }
    // An instance-disabled or unknown plugin must be indistinguishable.
    let Some(plugin) = state.plugins().get(&id) else {
        return text(StatusCode::NOT_FOUND, "no such plugin");
    };
    let Some(backend) = plugin.manifest.backend.clone() else {
        return text(StatusCode::NOT_FOUND, "this plugin has no backend");
    };
    if !caller_enabled(&state, &ctx, &id).await {
        return text(StatusCode::FORBIDDEN, "you have not enabled this plugin");
    }

    let settings = match crate::plugin_settings::resolved(state.pool(), &plugin.manifest).await {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("db error: {e}");
            return text(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    };
    let Some(upstream) = settings.get(&backend.upstream_setting) else {
        return text(
            StatusCode::SERVICE_UNAVAILABLE,
            "this plugin's backend URL has not been configured by an admin",
        );
    };
    let secret = match crate::plugin_settings::proxy_secret(state.pool(), &id).await {
        Ok(Some(secret)) => secret,
        Ok(None) => {
            return text(
                StatusCode::SERVICE_UNAVAILABLE,
                "this plugin has no proxy secret; rotate it in Settings → Plugins",
            );
        }
        Err(e) => {
            tracing::error!("db error: {e}");
            return text(StatusCode::INTERNAL_SERVER_ERROR, "database error");
        }
    };
    let user_name = user_name(&state, &ctx).await;

    let (parts, body) = request.into_parts();
    let query = parts.uri.query().map(str::to_owned);
    let signed = signed_path(&path);
    let ts = chrono::Utc::now().timestamp();
    let user_id = ctx.user_id.to_string();
    let signature = sign(&secret, parts.method.as_str(), &signed, ts, &user_id);
    let url = upstream_url(upstream, &signed, query.as_deref());

    let sent = client()
        .request(parts.method.clone(), &url)
        .headers(forwarded_headers(&parts.headers, &id, &user_id, &user_name, ts, &signature))
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await;
    let upstream = match sent {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(plugin = %id, "plugin backend unreachable: {e}");
            return text(StatusCode::BAD_GATEWAY, "the plugin's backend did not answer");
        }
    };

    let mut response = Response::builder().status(upstream.status());
    for (name, value) in upstream.headers() {
        if !response_strip(name.as_str()) {
            response = response.header(name, value);
        }
    }
    response
        .body(Body::from_stream(upstream.bytes_stream()))
        .unwrap_or_else(|_| text(StatusCode::BAD_GATEWAY, "malformed response from the backend"))
}

async fn caller_enabled(state: &impl ExtrasContext, ctx: &AuthContext, id: &str) -> bool {
    let settings: Option<serde_json::Value> =
        match sqlx::query_scalar("SELECT data FROM user_settings WHERE user_id = $1")
            .bind(ctx.user_id)
            .fetch_optional(state.pool())
            .await
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("db error: {e}");
                return false;
            }
        };
    crate::plugins::enabled_ids(settings.as_ref()).iter().any(|e| e == id)
}

async fn user_name(state: &impl ExtrasContext, ctx: &AuthContext) -> String {
    sqlx::query_scalar::<_, String>("SELECT name FROM users WHERE id = $1")
        .bind(ctx.user_id)
        .fetch_optional(state.pool())
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{
        PLUGIN_HEADER, SIG_HEADER, TS_HEADER, USER_ID_HEADER, USER_NAME_HEADER, canonical,
        forwarded_headers, response_strip, sign, signed_path, strip, upstream_url,
    };
    use axum::http::header::{HeaderMap, HeaderValue};

    #[test]
    fn the_canonical_string_is_method_path_ts_user_joined_by_newlines() {
        assert_eq!(
            canonical("GET", "/v1/pulls", 1_700_000_000, "user-1"),
            "GET\n/v1/pulls\n1700000000\nuser-1"
        );
    }

    #[test]
    fn the_signed_path_drops_the_query_and_normalises_the_leading_slash() {
        assert_eq!(signed_path("v1/pulls"), "/v1/pulls");
        assert_eq!(signed_path("/v1/pulls"), "/v1/pulls");
        assert_eq!(signed_path("//v1/pulls"), "/v1/pulls");
        assert_eq!(signed_path("v1/pulls?a=1&b=2"), "/v1/pulls");
        assert_eq!(signed_path("v1/pulls#frag"), "/v1/pulls");
        assert_eq!(signed_path(""), "/");
    }

    #[test]
    fn signatures_are_stable_and_change_with_every_field() {
        let base = sign("sekrit", "GET", "/v1/pulls", 1_700_000_000, "user-1");
        assert_eq!(base.len(), 64);
        assert_eq!(base, sign("sekrit", "GET", "/v1/pulls", 1_700_000_000, "user-1"));
        for other in [
            sign("other", "GET", "/v1/pulls", 1_700_000_000, "user-1"),
            sign("sekrit", "POST", "/v1/pulls", 1_700_000_000, "user-1"),
            sign("sekrit", "GET", "/v1/repos", 1_700_000_000, "user-1"),
            sign("sekrit", "GET", "/v1/pulls", 1_700_000_001, "user-1"),
            sign("sekrit", "GET", "/v1/pulls", 1_700_000_000, "user-2"),
        ] {
            assert_ne!(base, other);
        }
    }

    /// The vectors the ghreview verifier's own test reads, so the Rust signer
    /// and the TypeScript verifier cannot drift apart silently.
    #[test]
    fn the_shared_test_vectors_match_this_signer() {
        let raw = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/plugin-proxy-signature-vectors.json"
        ))
        .expect("shared signature vectors");
        let doc: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let vectors = doc["vectors"].as_array().expect("vectors array");
        assert!(!vectors.is_empty());
        for v in vectors {
            let s = |k: &str| v[k].as_str().unwrap().to_owned();
            let ts = v["ts"].as_i64().unwrap();
            assert_eq!(
                canonical(&s("method"), &s("path"), ts, &s("userId")),
                s("canonical"),
                "canonical string for {}",
                s("name")
            );
            assert_eq!(
                sign(&s("secret"), &s("method"), &s("path"), ts, &s("userId")),
                s("signature"),
                "signature for {}",
                s("name")
            );
        }
    }

    #[test]
    fn upstream_urls_join_without_doubling_or_dropping_segments() {
        assert_eq!(
            upstream_url("https://gh.example", "/v1/pulls", Some("state=open")),
            "https://gh.example/v1/pulls?state=open"
        );
        assert_eq!(
            upstream_url("https://gh.example/", "/v1/pulls", None),
            "https://gh.example/v1/pulls"
        );
        assert_eq!(
            upstream_url("https://gh.example/api", "/v1/x", None),
            "https://gh.example/api/v1/x"
        );
        assert_eq!(upstream_url("https://gh.example", "/", Some("")), "https://gh.example/");
    }

    #[test]
    fn the_hop_drops_cctui_credentials_and_client_supplied_identity_headers() {
        let mut src = HeaderMap::new();
        src.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("cctui_auth=secret; cctui_preview=t; theirs=1"),
        );
        src.insert(axum::http::header::AUTHORIZATION, HeaderValue::from_static("Bearer tok"));
        src.insert(axum::http::header::HOST, HeaderValue::from_static("cctui.example"));
        src.insert(axum::http::header::ACCEPT, HeaderValue::from_static("text/event-stream"));
        src.insert(USER_ID_HEADER, HeaderValue::from_static("spoofed"));
        src.insert(SIG_HEADER, HeaderValue::from_static("deadbeef"));

        let out = forwarded_headers(&src, "ghreview", "user-1", "Dorsk", 42, "abc123");
        assert_eq!(out.get(axum::http::header::COOKIE).unwrap(), "theirs=1");
        assert!(out.get(axum::http::header::AUTHORIZATION).is_none());
        assert!(out.get(axum::http::header::HOST).is_none());
        assert_eq!(out.get(axum::http::header::ACCEPT).unwrap(), "text/event-stream");
        assert_eq!(out.get(USER_ID_HEADER).unwrap(), "user-1");
        assert_eq!(out.get(USER_NAME_HEADER).unwrap(), "Dorsk");
        assert_eq!(out.get(PLUGIN_HEADER).unwrap(), "ghreview");
        assert_eq!(out.get(TS_HEADER).unwrap(), "42");
        assert_eq!(out.get(SIG_HEADER).unwrap(), "abc123");
    }

    #[test]
    fn a_request_with_only_cctui_cookies_forwards_no_cookie_header_at_all() {
        let mut src = HeaderMap::new();
        src.insert(
            axum::http::header::COOKIE,
            HeaderValue::from_static("cctui_auth=secret; cctui_preview=t"),
        );
        let out = forwarded_headers(&src, "p", "u", "n", 1, "s");
        assert!(out.get(axum::http::header::COOKIE).is_none());
    }

    #[test]
    fn hop_by_hop_headers_are_dropped_in_both_directions() {
        for name in ["connection", "transfer-encoding", "upgrade", "content-length"] {
            assert!(strip(name), "{name}");
            assert!(response_strip(name), "{name}");
        }
        assert!(strip("x-cctui-user-id"));
        assert!(!strip("accept"));
        assert!(!response_strip("content-type"));
        assert!(!response_strip("cache-control"));
    }
}
