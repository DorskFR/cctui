//! Minimal HTTP health endpoint on its own listener (`:15002` by default) for
//! container probes. `/health` and `/ready` return `200 OK`; `/dispatch/env`
//! hands a dispatched worker its payload env; everything else `404`.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::dispatch::DispatchIdentity;
use crate::policy::PolicyManager;

pub async fn serve(
    addr: &str,
    policy: Arc<PolicyManager>,
    dispatch: Option<Arc<DispatchIdentity>>,
) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, _peer) = listener.accept().await?;
        let policy = policy.clone();
        let dispatch = dispatch.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, &policy, dispatch.as_deref()).await {
                tracing::debug!("health connection ended: {e}");
            }
        });
    }
}

fn json_response(status: u16, body: &str) -> Vec<u8> {
    let reason = match status {
        200 => "OK",
        404 => "Not Found",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Status",
    };
    format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

async fn handle(
    mut conn: TcpStream,
    policy: &PolicyManager,
    dispatch: Option<&DispatchIdentity>,
) -> anyhow::Result<()> {
    let mut buf = [0u8; 1024];
    let n = conn.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    let path = text.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("");

    if path == "/dispatch/env" {
        let (status, body) = match dispatch {
            Some(d) => d.env().await,
            None => (404, r#"{"error":"this pod has no dispatch identity"}"#.to_owned()),
        };
        conn.write_all(&json_response(status, &body)).await?;
        return Ok(());
    }

    // `/health` is liveness (the process is up). `/ready` is readiness: it only
    // reports 200 once a policy is actually loaded — until then the proxy is
    // deny-all, which is healthy but not "ready to allow traffic".
    let response: &[u8] = match path {
        "/health" => b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nOK",
        "/ready" if policy.is_loaded() => {
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nConnection: close\r\n\r\nReady"
        }
        "/ready" => {
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 9\r\nConnection: close\r\n\r\nNo policy"
        }
        _ => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
    };
    conn.write_all(response).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_policy() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        (dir, path)
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let (dir, path) = temp_policy();
        std::mem::forget(dir);
        let policy = Arc::new(PolicyManager::new(&path));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle(stream, &policy, None).await.unwrap();
        });

        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 128];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.ends_with("OK"));
    }

    async fn get(path: &str, dispatch: Option<Arc<DispatchIdentity>>) -> String {
        let (dir, policy_path) = temp_policy();
        std::mem::forget(dir);
        let policy = Arc::new(PolicyManager::new(&policy_path));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle(stream, &policy, dispatch.as_deref()).await.unwrap();
        });
        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
        let mut resp = String::new();
        conn.read_to_string(&mut resp).await.unwrap();
        resp
    }

    #[tokio::test]
    async fn dispatch_env_without_an_identity_is_404() {
        let resp = get("/dispatch/env", None).await;
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    }

    #[tokio::test]
    async fn a_failed_redeem_answers_the_worker_with_its_reason() {
        let identity = DispatchIdentity::redeem(crate::dispatch::DispatchConfig {
            dispatch_id: "d-1".to_owned(),
            server: crate::dispatch::ServerTarget::parse("http://127.0.0.1:9").unwrap(),
            token_path: "/nonexistent/cctui-token".into(),
        })
        .await;
        let resp = get("/dispatch/env", Some(Arc::new(identity))).await;
        assert!(resp.starts_with("HTTP/1.1 503"), "got: {resp}");
        assert!(resp.contains("/nonexistent/cctui-token"), "got: {resp}");
    }

    #[tokio::test]
    async fn unknown_path_returns_404() {
        let (dir, path) = temp_policy();
        std::mem::forget(dir);
        let policy = Arc::new(PolicyManager::new(&path));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            handle(stream, &policy, None).await.unwrap();
        });

        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /nope HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 128];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    }
}
