//! Outbound server-to-server calls. Every peer URL, the inviter's and the
//! joiner's alike, passes the SSRF guard on every request.

use std::sync::OnceLock;
use std::time::Duration;

use axum::http::StatusCode;
use uuid::Uuid;

use super::sig::{self, Seed};
use crate::state::AppState;

const TIMEOUT: Duration = Duration::from_secs(15);
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Debug)]
pub enum ClientError {
    Url(String),
    Network(String),
    TooLarge,
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Url(e) => write!(f, "peer url refused: {e}"),
            Self::Network(e) => write!(f, "network error: {e}"),
            Self::TooLarge => write!(f, "peer response too large"),
        }
    }
}

/// The DNS-free shape every peer URL must have.
pub fn shape_ok(raw: &str, allow_private: bool) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw).map_err(|_| "malformed url".to_owned())?;
    match url.scheme() {
        "https" => {}
        "http" if allow_private => {}
        _ => return Err("https required".into()),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("credentials in url".into());
    }
    if url.host_str().is_none() || url.query().is_some() || url.fragment().is_some() {
        return Err("unexpected url shape".into());
    }
    Ok(url)
}

/// Shape check plus, unless private peers are allowed, the public-address guard.
pub async fn check_url(state: &AppState, raw: &str) -> Result<reqwest::Url, String> {
    let allow_private = state.config.cctuiverse.allow_private;
    let url = shape_ok(raw, allow_private)?;
    if !allow_private {
        crate::outbound::validate_outbound_url(raw, &[]).await.map_err(|e| e.to_string())?;
    }
    Ok(url)
}

fn http(allow_private: bool) -> &'static reqwest::Client {
    static GUARDED: OnceLock<reqwest::Client> = OnceLock::new();
    static OPEN: OnceLock<reqwest::Client> = OnceLock::new();
    if allow_private {
        OPEN.get_or_init(|| {
            crate::install_crypto_provider();
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .no_proxy()
                .build()
                .expect("build cctuiverse client")
        })
    } else {
        GUARDED
            .get_or_init(|| crate::outbound::guarded_direct_client(crate::outbound::no_allowlist))
    }
}

/// `base_url` joined with `route` (which starts with `/`).
#[must_use]
pub fn join_url(base_url: &str, route: &str) -> String {
    format!("{}{route}", base_url.trim_end_matches('/'))
}

/// POST `body` to `base_url + route`, signed with `seed` under `keyid`.
pub async fn post_signed(
    state: &AppState,
    seed: &Seed,
    keyid: Uuid,
    base_url: &str,
    route: &str,
    body: &serde_json::Value,
) -> Result<(StatusCode, Vec<u8>), ClientError> {
    let raw = join_url(base_url, route);
    let allow_private = state.config.cctuiverse.allow_private;
    let url = shape_ok(&raw, allow_private).map_err(ClientError::Url)?;
    if !allow_private {
        match crate::outbound::validate_outbound_url(&raw, &[]).await {
            Ok(()) => {}
            Err(e @ crate::outbound::OutboundUrlError::Unresolvable) => {
                return Err(ClientError::Network(e.to_string()));
            }
            Err(e) => return Err(ClientError::Url(e.to_string())),
        }
    }
    let bytes = serde_json::to_vec(body).map_err(|e| ClientError::Network(e.to_string()))?;
    let signed = sig::sign(
        seed,
        "POST",
        url.path(),
        &bytes,
        keyid,
        chrono::Utc::now().timestamp(),
        &sig::random_b64url::<16>(),
    );
    let mut resp = http(allow_private)
        .post(url)
        .timeout(TIMEOUT)
        .header("content-type", "application/json")
        .header("content-digest", signed.content_digest)
        .header("signature-input", signed.signature_input)
        .header("signature", signed.signature)
        .body(bytes)
        .send()
        .await
        .map_err(|e| ClientError::Network(e.to_string()))?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    if resp.content_length().is_some_and(|n| n > MAX_RESPONSE_BYTES as u64) {
        return Err(ClientError::TooLarge);
    }
    let mut out = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| ClientError::Network(e.to_string()))? {
        out.extend_from_slice(&chunk);
        if out.len() > MAX_RESPONSE_BYTES {
            return Err(ClientError::TooLarge);
        }
    }
    Ok((status, out))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_urls_must_be_bare_https_unless_private_is_allowed() {
        assert!(shape_ok("https://b.example", false).is_ok());
        assert!(shape_ok("https://b.example:8443/cctui", false).is_ok());
        for bad in [
            "http://b.example",
            "ftp://b.example",
            "https://u:p@b.example",
            "https://u@b.example",
            "https://b.example/?x=1",
            "https://b.example/#frag",
            "not a url",
        ] {
            assert!(shape_ok(bad, false).is_err(), "{bad}");
        }
        assert!(shape_ok("http://10.0.0.5:8700", true).is_ok());
        assert!(shape_ok("http://u@10.0.0.5:8700", true).is_err());
    }

    #[test]
    fn routes_join_onto_a_sub_path_base() {
        assert_eq!(
            join_url("https://b.example/cctui/", "/cctuiverse/v1/join"),
            "https://b.example/cctui/cctuiverse/v1/join"
        );
    }
}
