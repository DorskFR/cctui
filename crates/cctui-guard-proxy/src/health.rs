//! Minimal HTTP endpoint on its own listener (`:15002` by default): `/health`
//! and `/ready` for probes, `/dispatch/env` for the dispatched worker, and on a
//! guard pod `/ca.pem` plus the control-token `PUT /policy` and
//! `PUT /private-allow` the worker steers it with.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::auth::GuardAuth;
use crate::dispatch::DispatchIdentity;
use crate::inject::header_value;
use crate::policy::{PolicyConfig, PolicyManager};

const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;

pub struct Health {
    pub policy: Arc<PolicyManager>,
    pub dispatch: Option<Arc<DispatchIdentity>>,
    pub auth: Arc<GuardAuth>,
    pub ca_pem: Option<String>,
    pub policy_path: PathBuf,
    pub private_allow_path: PathBuf,
}

pub async fn serve(addr: &str, health: Arc<Health>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, peer) = listener.accept().await?;
        let health = health.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, peer.ip(), &health).await {
                tracing::debug!("health connection ended: {e}");
            }
        });
    }
}

fn http_response(status: u16, content_type: &str, body: &str) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        413 => "Payload Too Large",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn json_response(status: u16, body: &str) -> Vec<u8> {
    http_response(status, "application/json", body)
}

/// The request head, and whatever of the body arrived with it.
async fn read_head(conn: &mut TcpStream) -> anyhow::Result<Option<(Vec<u8>, Vec<u8>)>> {
    let mut raw = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        let n = conn.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None);
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(end) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            let body = raw.split_off(end + 4);
            return Ok(Some((raw, body)));
        }
        anyhow::ensure!(raw.len() <= MAX_HEAD, "request head too large");
    }
}

async fn read_body(conn: &mut TcpStream, head: &[u8], mut body: Vec<u8>) -> Option<Vec<u8>> {
    let len: usize =
        std::str::from_utf8(header_value(head, "content-length")?).ok()?.parse().ok()?;
    if len > MAX_BODY || body.len() > len {
        return None;
    }
    let start = body.len();
    body.resize(len, 0);
    conn.read_exact(&mut body[start..]).await.ok()?;
    Some(body)
}

fn write_atomically(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("put.tmp");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

async fn put(
    conn: &mut TcpStream,
    health: &Health,
    path: &str,
    head: &[u8],
    body: Vec<u8>,
) -> anyhow::Result<()> {
    if !health.auth.control_ok(head) {
        conn.write_all(&json_response(403, r#"{"error":"control token required"}"#)).await?;
        return Ok(());
    }
    let Some(body) = read_body(conn, head, body).await else {
        conn.write_all(&json_response(413, r#"{"error":"body missing or too large"}"#)).await?;
        return Ok(());
    };
    let target = if path == "/policy" {
        if serde_json::from_slice::<PolicyConfig>(&body).is_err() {
            conn.write_all(&json_response(400, r#"{"error":"not a policy document"}"#)).await?;
            return Ok(());
        }
        &health.policy_path
    } else {
        if std::str::from_utf8(&body).is_err() {
            conn.write_all(&json_response(400, r#"{"error":"not UTF-8"}"#)).await?;
            return Ok(());
        }
        &health.private_allow_path
    };
    if let Err(e) = write_atomically(target, &body) {
        tracing::error!("writing {}: {e}", target.display());
        conn.write_all(&json_response(500, r#"{"error":"write failed"}"#)).await?;
        return Ok(());
    }
    tracing::info!("{path} replaced by the worker ({} bytes)", body.len());
    conn.write_all(&json_response(200, "{}")).await?;
    Ok(())
}

async fn dispatch_env(
    conn: &mut TcpStream,
    peer: IpAddr,
    health: &Health,
    head: &[u8],
) -> anyhow::Result<()> {
    if !peer.to_canonical().is_loopback() && !health.auth.worker_ok(head) {
        let body = r#"{"error":"dispatch env is only served to the pod's loopback or worker"}"#;
        conn.write_all(&json_response(403, body)).await?;
        return Ok(());
    }
    let Some(dispatch) = &health.dispatch else {
        let body = r#"{"error":"this pod has no dispatch identity"}"#;
        conn.write_all(&json_response(404, body)).await?;
        return Ok(());
    };
    let (status, body) = Box::pin(dispatch.env()).await;
    conn.write_all(&json_response(status, &body)).await?;
    Ok(())
}

async fn handle(mut conn: TcpStream, peer: IpAddr, health: &Health) -> anyhow::Result<()> {
    let Some((head, body)) = read_head(&mut conn).await? else {
        return Ok(());
    };
    let text = String::from_utf8_lossy(&head);
    let mut line = text.lines().next().unwrap_or_default().split_whitespace();
    let (method, path) = (line.next().unwrap_or_default(), line.next().unwrap_or_default());

    match (method, path) {
        ("PUT", "/policy" | "/private-allow") => {
            return Box::pin(put(&mut conn, health, path, &head, body)).await;
        }
        (_, "/dispatch/env") => {
            return Box::pin(dispatch_env(&mut conn, peer, health, &head)).await;
        }
        (_, "/ca.pem") => {
            let resp = health.ca_pem.as_deref().map_or_else(
                || json_response(404, r#"{"error":"no per-pod CA: injection is off"}"#),
                |pem| http_response(200, "application/x-pem-file", pem),
            );
            conn.write_all(&resp).await?;
            return Ok(());
        }
        _ => {}
    }

    // `/health` is liveness (the process is up). `/ready` is readiness: it only
    // reports 200 once a policy is actually loaded — until then the proxy is
    // deny-all, which is healthy but not "ready to allow traffic".
    let response = match path {
        "/health" => http_response(200, "text/plain", "OK"),
        "/ready" => ready_response(&health.policy),
        _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    };
    conn.write_all(&response).await?;
    Ok(())
}

/// The guard's `/transition` waits on `generation` to know its policy is live.
fn ready_response(policy: &PolicyManager) -> Vec<u8> {
    let generation = policy.generation();
    let body = serde_json::json!({ "ready": generation.is_some(), "generation": generation });
    json_response(if generation.is_some() { 200 } else { 503 }, &body.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOPBACK: IpAddr = IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    const PROXY_TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const CONTROL_TOKEN: &str = "fedcba9876543210fedcba9876543210";

    fn health(
        dir: &Path,
        dispatch: Option<Arc<DispatchIdentity>>,
        auth: GuardAuth,
        ca_pem: Option<&str>,
    ) -> Arc<Health> {
        let policy_path = dir.join("policy.json");
        Arc::new(Health {
            policy: Arc::new(PolicyManager::new(&policy_path)),
            dispatch,
            auth: Arc::new(auth),
            ca_pem: ca_pem.map(ToOwned::to_owned),
            policy_path,
            private_allow_path: dir.join("private-allow"),
        })
    }

    async fn send(peer: IpAddr, health: Arc<Health>, request: &str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle(stream, peer, &health).await.unwrap();
        });
        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(request.as_bytes()).await.unwrap();
        let mut resp = String::new();
        conn.read_to_string(&mut resp).await.unwrap();
        resp
    }

    async fn get_from(peer: IpAddr, path: &str, dispatch: Option<Arc<DispatchIdentity>>) -> String {
        let dir = tempfile::tempdir().unwrap();
        let state = health(dir.path(), dispatch, GuardAuth::default(), None);
        send(peer, state, &format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n")).await
    }

    async fn get(path: &str, dispatch: Option<Arc<DispatchIdentity>>) -> String {
        get_from(LOOPBACK, path, dispatch).await
    }

    async fn failed_identity() -> Option<Arc<DispatchIdentity>> {
        let identity = DispatchIdentity::redeem(crate::dispatch::DispatchConfig {
            dispatch_id: "d-1".to_owned(),
            server: crate::dispatch::ServerTarget::parse("http://127.0.0.1:9").unwrap(),
            token_path: "/nonexistent/cctui-token".into(),
        })
        .await;
        Some(Arc::new(identity))
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let resp = get("/health", None).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.ends_with("OK"));
    }

    #[tokio::test]
    async fn dispatch_env_without_an_identity_is_404() {
        let resp = get("/dispatch/env", None).await;
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    }

    #[tokio::test]
    async fn dispatch_env_is_refused_to_a_non_loopback_peer() {
        let identity = failed_identity().await;
        let pod_peer: IpAddr = "10.42.1.7".parse().unwrap();
        let resp = get_from(pod_peer, "/dispatch/env", identity.clone()).await;
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
        let mapped: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        let resp = get_from(mapped, "/dispatch/env", identity).await;
        assert!(resp.starts_with("HTTP/1.1 503"), "got: {resp}");
    }

    #[tokio::test]
    async fn a_guard_pod_serves_dispatch_env_to_its_worker_token_only() {
        let dir = tempfile::tempdir().unwrap();
        let auth = GuardAuth::with_tokens(Some(PROXY_TOKEN), Some(CONTROL_TOKEN));
        let state = health(dir.path(), failed_identity().await, auth, None);
        let worker: IpAddr = "10.42.1.7".parse().unwrap();
        let ask = |token: &str| {
            format!("GET /dispatch/env HTTP/1.1\r\nAuthorization: Bearer {token}\r\n\r\n")
        };
        let resp = send(worker, state.clone(), &ask(PROXY_TOKEN)).await;
        assert!(resp.starts_with("HTTP/1.1 503"), "got: {resp}");
        let resp = send(worker, state.clone(), &ask(CONTROL_TOKEN)).await;
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
        let resp = send(worker, state, "GET /dispatch/env HTTP/1.1\r\nHost: g\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
    }

    #[tokio::test]
    async fn a_failed_redeem_answers_the_worker_with_its_reason() {
        let resp = get("/dispatch/env", failed_identity().await).await;
        assert!(resp.starts_with("HTTP/1.1 503"), "got: {resp}");
        assert!(resp.contains("/nonexistent/cctui-token"), "got: {resp}");
    }

    #[tokio::test]
    async fn ca_pem_is_served_when_the_pod_minted_a_ca() {
        let dir = tempfile::tempdir().unwrap();
        let pem = "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n";
        let state = health(dir.path(), None, GuardAuth::default(), Some(pem));
        let resp = send(LOOPBACK, state, "GET /ca.pem HTTP/1.1\r\nHost: g\r\n\r\n").await;
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.ends_with(pem), "got: {resp}");
        let resp = get("/ca.pem", None).await;
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    }

    fn put_request(path: &str, token: &str, body: &str) -> String {
        format!(
            "PUT {path} HTTP/1.1\r\nHost: g\r\nAuthorization: Bearer {token}\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    #[tokio::test]
    async fn the_control_token_replaces_the_policy_and_the_proxy_token_cannot() {
        let dir = tempfile::tempdir().unwrap();
        let auth = GuardAuth::with_tokens(Some(PROXY_TOKEN), Some(CONTROL_TOKEN));
        let state = health(dir.path(), None, auth, None);
        let worker: IpAddr = "10.42.1.7".parse().unwrap();
        let policy = r#"{"allowed_hosts":["a.test:443"],"default":"deny"}"#;

        let resp = send(worker, state.clone(), &put_request("/policy", PROXY_TOKEN, policy)).await;
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
        assert!(!dir.path().join("policy.json").exists());

        let resp = send(worker, state.clone(), &put_request("/policy", CONTROL_TOKEN, "[1]")).await;
        assert!(resp.starts_with("HTTP/1.1 400"), "got: {resp}");

        let request = put_request("/policy", CONTROL_TOKEN, policy);
        let resp = send(worker, state.clone(), &request).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert_eq!(std::fs::read_to_string(dir.path().join("policy.json")).unwrap(), policy);
        state.policy.load().unwrap();
        assert!(state.policy.is_allowed("a.test:443"));

        let lines = "cctui.cctui.svc:8700 CCTUI_URL\n";
        let request = put_request("/private-allow", CONTROL_TOKEN, lines);
        let resp = send(worker, state, &request).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert_eq!(std::fs::read_to_string(dir.path().join("private-allow")).unwrap(), lines);
    }

    #[tokio::test]
    async fn a_sidecar_without_a_control_token_cannot_be_steered() {
        let dir = tempfile::tempdir().unwrap();
        let state = health(dir.path(), None, GuardAuth::default(), None);
        let request = put_request("/policy", CONTROL_TOKEN, r#"{"default":"allow"}"#);
        let resp = send(LOOPBACK, state, &request).await;
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
        assert!(!dir.path().join("policy.json").exists());
    }

    fn ready_body(resp: &str) -> serde_json::Value {
        let (_, body) = resp.split_once("\r\n\r\n").unwrap();
        serde_json::from_str(body).unwrap()
    }

    #[tokio::test]
    async fn ready_reports_the_loaded_policy_generation() {
        let dir = tempfile::tempdir().unwrap();
        let state = health(dir.path(), None, GuardAuth::default(), None);
        let request = "GET /ready HTTP/1.1\r\nHost: g\r\n\r\n";

        let resp = send(LOOPBACK, state.clone(), request).await;
        assert!(resp.starts_with("HTTP/1.1 503"), "got: {resp}");
        assert_eq!(ready_body(&resp), serde_json::json!({ "ready": false, "generation": null }));

        std::fs::write(
            dir.path().join("policy.json"),
            r#"{"allowed_hosts":[],"default":"deny","generation":5}"#,
        )
        .unwrap();
        state.policy.load().unwrap();
        let resp = send(LOOPBACK, state.clone(), request).await;
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert_eq!(ready_body(&resp), serde_json::json!({ "ready": true, "generation": 5 }));

        std::fs::write(dir.path().join("policy.json"), r#"{"allowed_hosts":[],"default":"deny"}"#)
            .unwrap();
        state.policy.load().unwrap();
        let resp = send(LOOPBACK, state, request).await;
        assert_eq!(ready_body(&resp), serde_json::json!({ "ready": true, "generation": 0 }));
    }

    #[tokio::test]
    async fn unknown_path_returns_404() {
        let resp = get("/nope", None).await;
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    }
}
