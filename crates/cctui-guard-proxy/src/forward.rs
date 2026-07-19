//! Forward-proxy mode (new capability): a standard HTTP proxy for environments
//! where transparent capture isn't possible (rootless Docker, Apple container,
//! gVisor). The worker exports `HTTP_PROXY`/`HTTPS_PROXY` pointing here instead
//! of installing iptables rules.
//!
//! - `CONNECT host:port` → policy check → `200`, then blind TCP tunnel (TLS).
//! - absolute-URI request (`GET http://host/…`) → policy check → relay.
//!
//! Same policy evaluation as transparent mode, on the requested host:port.

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::denylist::host_is_denied;
use crate::inject::Injector;
use crate::policy::PolicyManager;

pub struct ForwardListener {
    listener: TcpListener,
    policy: Arc<PolicyManager>,
    injection: Option<Arc<Injector>>,
}

impl ForwardListener {
    pub async fn bind(
        addr: &str,
        policy: Arc<PolicyManager>,
        injection: Option<Arc<Injector>>,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener, policy, injection })
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        loop {
            let (stream, _peer) = self.listener.accept().await?;
            let policy = self.policy.clone();
            let injection = self.injection.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, policy, injection).await {
                    tracing::debug!("forward connection ended: {e}");
                }
            });
        }
    }
}

/// A parsed proxy request: just enough of the request line + headers to make a
/// policy decision and relay.
struct Request {
    method: String,
    target: String,
    /// The full received bytes (request line + headers + any pipelined body
    /// that arrived in the same read), so plain-HTTP relay can replay them.
    raw: Vec<u8>,
}

/// Reads bytes until the end of the HTTP header block (`\r\n\r\n`), then parses
/// the request line. Caps the header size to avoid unbounded buffering.
async fn read_request(conn: &mut TcpStream) -> anyhow::Result<Option<Request>> {
    let mut raw = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        let n = conn.read(&mut chunk).await?;
        if n == 0 {
            return Ok(None); // client closed before sending a full request
        }
        raw.extend_from_slice(&chunk[..n]);
        if find_header_end(&raw).is_some() {
            break;
        }
        if raw.len() > 64 * 1024 {
            anyhow::bail!("request header too large");
        }
    }

    let text = String::from_utf8_lossy(&raw);
    let request_line = text.lines().next().unwrap_or_default();
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    if method.is_empty() || target.is_empty() {
        anyhow::bail!("malformed request line");
    }

    Ok(Some(Request { method, target, raw }))
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4).position(|w| w == b"\r\n\r\n")
}

async fn handle_connection(
    mut conn: TcpStream,
    policy: Arc<PolicyManager>,
    injection: Option<Arc<Injector>>,
) -> anyhow::Result<()> {
    let Some(req) = read_request(&mut conn).await? else {
        return Ok(());
    };

    if req.method.eq_ignore_ascii_case("CONNECT") {
        handle_connect(conn, &req.target, &policy, injection.as_deref()).await
    } else {
        handle_http(conn, &req, &policy).await
    }
}

/// `CONNECT host:port` — the target is already a host:port authority.
// Linear deny/allow + dial + splice flow; the complexity is the per-step HTTP
// error responses (`write_all().await?`), not nesting. Splitting would scatter
// the security-critical allow/deny control flow across helpers.
#[allow(clippy::cognitive_complexity)]
async fn handle_connect(
    mut conn: TcpStream,
    target: &str,
    policy: &PolicyManager,
    injection: Option<&Injector>,
) -> anyhow::Result<()> {
    let host_port = normalize_authority(target, 443);

    if host_is_denied(&split_host_port(&host_port, 443).0) {
        tracing::warn!("DENY (builtin) CONNECT {host_port}");
        conn.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }

    if !policy.is_allowed(&host_port) {
        tracing::info!("DENY CONNECT {host_port}");
        conn.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    tracing::info!("ALLOW CONNECT {host_port}");

    let (host, port) = split_host_port(&host_port, 443);

    if let Some(injector) = injection
        && injector.should_inject(&host)
    {
        tracing::info!("INJECT CONNECT {host}:{port}");
        conn.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
        return Box::pin(injector.handle(conn, Vec::new(), &host, port)).await;
    }

    let Ok(Ok(mut upstream)) =
        tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&host_port)).await
    else {
        tracing::warn!("CONNECT dial {host_port} failed");
        conn.write_all(
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await?;
        return Ok(());
    };

    conn.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?;
    tokio::io::copy_bidirectional(&mut conn, &mut upstream).await?;
    Ok(())
}

/// Splits a `host:port` authority into host and port, defaulting the port and
/// handling bracketed IPv6 literals.
fn split_host_port(host_port: &str, default_port: u16) -> (String, u16) {
    if let Some(rest) = host_port.strip_prefix('[') {
        if let Some(idx) = rest.find("]:") {
            let port = rest[idx + 2..].parse().unwrap_or(default_port);
            return (rest[..idx].to_owned(), port);
        }
        return (rest.trim_end_matches(']').to_owned(), default_port);
    }
    host_port.rsplit_once(':').map_or_else(
        || (host_port.to_owned(), default_port),
        |(h, p)| (h.to_owned(), p.parse().unwrap_or(default_port)),
    )
}

/// Plain HTTP with an absolute-URI request target (`GET http://host/path …`).
// Linear parse/deny/allow + dial + splice flow; complexity is the per-step HTTP
// error responses, not nesting. Splitting would scatter the allow/deny control flow.
#[allow(clippy::cognitive_complexity)]
async fn handle_http(
    mut conn: TcpStream,
    req: &Request,
    policy: &PolicyManager,
) -> anyhow::Result<()> {
    let Some((host_port, _path)) = split_absolute_uri(&req.target) else {
        tracing::info!("DENY HTTP (non-absolute target {})", req.target);
        conn.write_all(
            b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await?;
        return Ok(());
    };

    if host_is_denied(&split_host_port(&host_port, 80).0) {
        tracing::warn!("DENY (builtin) HTTP {} {}", req.method, req.target);
        conn.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }

    if !policy.is_allowed(&host_port) {
        tracing::info!("DENY HTTP {} {}", req.method, req.target);
        conn.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .await?;
        return Ok(());
    }
    tracing::info!("ALLOW HTTP {} {}", req.method, req.target);

    let Ok(Ok(mut upstream)) =
        tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(&host_port)).await
    else {
        tracing::warn!("HTTP dial {host_port} failed");
        conn.write_all(
            b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        )
        .await?;
        return Ok(());
    };

    // Relay the request as received (origin servers tolerate absolute-form
    // request targets), then splice the response and any further bytes.
    upstream.write_all(&req.raw).await?;
    tokio::io::copy_bidirectional(&mut conn, &mut upstream).await?;
    Ok(())
}

/// Ensures an authority carries a port, defaulting when absent. Leaves bracketed
/// IPv6 literals (`[::1]:443`) intact.
fn normalize_authority(authority: &str, default_port: u16) -> String {
    if authority.starts_with('[') {
        if authority.contains("]:") {
            return authority.to_string();
        }
        return format!("{authority}:{default_port}");
    }
    if authority.contains(':') {
        return authority.to_string();
    }
    format!("{authority}:{default_port}")
}

/// Splits an absolute-URI request target into (`host:port`, path). Only `http`
/// is handled in plain mode (TLS goes through CONNECT). Returns `None` if the
/// target isn't an `http://` absolute URI.
fn split_absolute_uri(target: &str) -> Option<(String, String)> {
    let rest = target.strip_prefix("http://")?;
    let (authority, path) =
        rest.split_once('/').map_or_else(|| (rest, "/".to_string()), |(a, p)| (a, format!("/{p}")));
    Some((normalize_authority(authority, 80), path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;

    fn write_policy(json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        (dir, path)
    }

    async fn start_forward(policy_json: &str) -> std::net::SocketAddr {
        let (dir, path) = write_policy(policy_json);
        // Keep the tempdir alive for the process lifetime of the test.
        std::mem::forget(dir);
        let policy = Arc::new(PolicyManager::new(&path));
        policy.load().unwrap();
        let listener = ForwardListener::bind("127.0.0.1:0", policy, None).await.unwrap();
        let addr = listener.listener.local_addr().unwrap();
        tokio::spawn(async move { listener.serve().await });
        addr
    }

    /// Spins up a one-shot TCP echo server and returns its address.
    async fn start_echo() -> std::net::SocketAddr {
        let ln = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = ln.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut s, _) = ln.accept().await.unwrap();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    loop {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                if s.write_all(&buf[..n]).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                });
            }
        });
        addr
    }

    #[test]
    fn normalize_authority_adds_default_port() {
        assert_eq!(normalize_authority("example.com", 443), "example.com:443");
        assert_eq!(normalize_authority("example.com:8443", 443), "example.com:8443");
        assert_eq!(normalize_authority("[::1]", 443), "[::1]:443");
        assert_eq!(normalize_authority("[::1]:80", 443), "[::1]:80");
    }

    #[test]
    fn split_absolute_uri_cases() {
        assert_eq!(
            split_absolute_uri("http://example.com/path"),
            Some(("example.com:80".to_string(), "/path".to_string()))
        );
        assert_eq!(
            split_absolute_uri("http://example.com:8080/"),
            Some(("example.com:8080".to_string(), "/".to_string()))
        );
        assert_eq!(split_absolute_uri("/relative"), None);
        assert_eq!(split_absolute_uri("https://example.com/"), None);
    }

    #[tokio::test]
    async fn connect_allowed_tunnels() {
        let echo = start_echo().await;
        let policy_json = format!(r#"{{"allowed_hosts": ["{echo}"], "default": "deny"}}"#);
        let proxy = start_forward(&policy_json).await;

        let mut conn = TcpStream::connect(proxy).await.unwrap();
        conn.write_all(format!("CONNECT {echo} HTTP/1.1\r\nHost: {echo}\r\n\r\n").as_bytes())
            .await
            .unwrap();

        let mut buf = [0u8; 64];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");

        // Tunnel is live: echo round-trips.
        conn.write_all(b"ping").await.unwrap();
        let mut echo_buf = [0u8; 4];
        conn.read_exact(&mut echo_buf).await.unwrap();
        assert_eq!(&echo_buf, b"ping");
    }

    #[tokio::test]
    async fn connect_denied_returns_403() {
        let proxy = start_forward(r#"{"allowed_hosts": [], "default": "deny"}"#).await;

        let mut conn = TcpStream::connect(proxy).await.unwrap();
        conn.write_all(
            b"CONNECT evil.example.com:443 HTTP/1.1\r\nHost: evil.example.com:443\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = [0u8; 64];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
    }

    #[tokio::test]
    async fn builtin_deny_overrides_allowlist_connect() {
        // Even with default=allow AND the metadata IP explicitly allowlisted,
        // the built-in deny refuses the credential endpoint (CCT-720).
        let proxy =
            start_forward(r#"{"allowed_hosts": ["169.254.169.254:443"], "default": "allow"}"#)
                .await;

        let mut conn = TcpStream::connect(proxy).await.unwrap();
        conn.write_all(b"CONNECT 169.254.169.254:443 HTTP/1.1\r\nHost: 169.254.169.254\r\n\r\n")
            .await
            .unwrap();

        let mut buf = [0u8; 64];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
    }

    #[tokio::test]
    async fn absolute_uri_denied_returns_403() {
        let proxy = start_forward(r#"{"allowed_hosts": [], "default": "deny"}"#).await;

        let mut conn = TcpStream::connect(proxy).await.unwrap();
        conn.write_all(
            b"GET http://evil.example.com/secret HTTP/1.1\r\nHost: evil.example.com\r\n\r\n",
        )
        .await
        .unwrap();

        let mut buf = [0u8; 64];
        let n = conn.read(&mut buf).await.unwrap();
        let resp = String::from_utf8_lossy(&buf[..n]);
        assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
    }

    #[tokio::test]
    async fn absolute_uri_allowed_relays() {
        let echo = start_echo().await;
        let policy_json = format!(r#"{{"allowed_hosts": ["{echo}"], "default": "deny"}}"#);
        let proxy = start_forward(&policy_json).await;

        let mut conn = TcpStream::connect(proxy).await.unwrap();
        // The echo server reflects whatever we send, so the "response" is the
        // raw request bytes coming back — confirming the relay reached upstream.
        let request = format!("GET http://{echo}/ HTTP/1.1\r\nHost: {echo}\r\n\r\n");
        conn.write_all(request.as_bytes()).await.unwrap();

        let mut buf = [0u8; 256];
        let n = conn.read(&mut buf).await.unwrap();
        let echoed = String::from_utf8_lossy(&buf[..n]);
        assert!(echoed.starts_with("GET http://"), "got: {echoed}");
    }
}
