//! Minimal HTTP health endpoint on its own listener (`:15002` by default) for
//! container probes. `/health` and `/ready` return `200 OK`; everything else
//! `404`.

use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::policy::PolicyManager;

pub async fn serve(addr: &str, policy: Arc<PolicyManager>) -> anyhow::Result<()> {
    let listener = TcpListener::bind(addr).await?;
    loop {
        let (stream, _peer) = listener.accept().await?;
        let policy = policy.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, &policy).await {
                tracing::debug!("health connection ended: {e}");
            }
        });
    }
}

async fn handle(mut conn: TcpStream, policy: &PolicyManager) -> anyhow::Result<()> {
    let mut buf = [0u8; 1024];
    let n = conn.read(&mut buf).await?;
    if n == 0 {
        return Ok(());
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    let path = text.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("");

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
            handle(stream, &policy).await.unwrap();
        });

        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /health HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 128];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
        assert!(resp.ends_with("OK"));
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
            handle(stream, &policy).await.unwrap();
        });

        let mut conn = TcpStream::connect(addr).await.unwrap();
        conn.write_all(b"GET /nope HTTP/1.1\r\nHost: x\r\n\r\n").await.unwrap();
        let mut buf = [0u8; 128];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
    }
}
