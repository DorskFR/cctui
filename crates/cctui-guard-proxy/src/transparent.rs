//! Transparent proxy mode: the iptables REDIRECT target. Connections arrive as
//! raw bytes (a TLS `ClientHello` or a plaintext HTTP request), not as an HTTP
//! CONNECT. We recover the original destination via `SO_ORIGINAL_DST`, enforce
//! policy on the recovered SNI / Host name, then tunnel.

use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use nix::sys::socket::getsockopt;
use nix::sys::socket::sockopt::OriginalDst;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::denylist::{host_is_denied, ip_is_denied};
use crate::inject::Injector;
use crate::peek::{extract_http_host, extract_sni};
use crate::policy::PolicyManager;
use crate::private_allow::PrivateAllow;

/// Recovers the pre-`DNAT` destination of a `REDIRECT`ed socket via the
/// `SO_ORIGINAL_DST` getsockopt. Uses the safe `nix` wrapper (no `unsafe`).
fn original_dst(stream: &TcpStream) -> anyhow::Result<SocketAddrV4> {
    let sa = getsockopt(stream, OriginalDst)?;
    // `sockaddr_in` fields are network byte order.
    let port = u16::from_be(sa.sin_port);
    let ip = Ipv4Addr::from(u32::from_be(sa.sin_addr.s_addr));
    Ok(SocketAddrV4::new(ip, port))
}

/// Listens for `REDIRECT`ed connections and enforces policy on each.
pub struct TransparentListener {
    listener: TcpListener,
    policy: Arc<PolicyManager>,
    injection: Option<Arc<Injector>>,
    private: Arc<PrivateAllow>,
}

impl TransparentListener {
    pub async fn bind(
        addr: &str,
        policy: Arc<PolicyManager>,
        injection: Option<Arc<Injector>>,
        private: Arc<PrivateAllow>,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        Ok(Self { listener, policy, injection, private })
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        loop {
            let (stream, _peer) = self.listener.accept().await?;
            let policy = self.policy.clone();
            let injection = self.injection.clone();
            let private = self.private.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_connection(stream, policy, injection, private).await {
                    tracing::debug!("transparent connection ended: {e}");
                }
            });
        }
    }
}

/// The first bytes of a redirected connection and the name they carry.
struct Peeked {
    buf: Vec<u8>,
    sni: Option<String>,
    http_host: Option<String>,
}

/// Peeks the first bytes to recover the intended hostname: the policy is a
/// hostname allow-list, so matching on the `SO_ORIGINAL_DST` IP would never hit.
async fn peek_name(conn: &mut TcpStream) -> anyhow::Result<Peeked> {
    let mut buf = vec![0u8; 4096];
    let n = match tokio::time::timeout(Duration::from_secs(2), conn.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => 0, // read timed out: no name recovered, fall through to deny
    };
    buf.truncate(n);
    let sni = extract_sni(&buf);
    let http_host = if sni.is_none() { extract_http_host(&buf) } else { None };
    Ok(Peeked { buf, sni, http_host })
}

// Linear name-recovery → fail-closed policy check → dial → splice flow; the
// complexity is the per-step error branches, not nesting.
#[allow(clippy::cognitive_complexity)]
async fn handle_connection(
    mut conn: TcpStream,
    policy: Arc<PolicyManager>,
    injection: Option<Arc<Injector>>,
    private: Arc<PrivateAllow>,
) -> anyhow::Result<()> {
    let dst = original_dst(&conn)?;
    let host_port = dst.to_string();
    let port = dst.port();
    let dst_ip = IpAddr::V4(*dst.ip());
    let Peeked { buf, sni, http_host } = peek_name(&mut conn).await?;

    // Without a recovered name the target is the original-destination IP:port,
    // which then fails a hostname allow-list: fail closed.
    let name = sni.as_deref().or(http_host.as_deref());
    let host = name.map_or_else(|| dst.ip().to_string(), str::to_owned);
    let policy_target = format!("{host}:{port}");

    let exempt = private.exempts(&host, port);
    if ip_is_denied(dst_ip, exempt) || name.is_some_and(|n| host_is_denied(n, exempt)) {
        tracing::warn!(
            "DENY (builtin) transparent {policy_target} (orig={host_port} sni={sni:?} host={http_host:?})"
        );
        policy.record(&policy_target, false, "builtin denylist");
        return Ok(());
    }

    if !policy.is_allowed(&policy_target) {
        tracing::info!(
            "DENY transparent {policy_target} (orig={host_port} sni={sni:?} host={http_host:?})"
        );
        policy.record(&policy_target, false, "not in allow-list");
        return Ok(());
    }

    tracing::info!(
        "ALLOW transparent {policy_target} (orig={host_port} sni={sni:?} host={http_host:?})"
    );
    policy.record(&policy_target, true, "");

    // Dial the proxy's own resolution of the name, never the worker-supplied
    // SO_ORIGINAL_DST; a private original destination must be one of its addresses.
    let addrs = match private.resolve(&host, port, Some(dst_ip)).await {
        Ok(addrs) => addrs,
        Err(e) => {
            tracing::warn!("resolve {policy_target} failed (orig={host_port}): {e}");
            policy.record(&policy_target, false, "resolve/denylist");
            return Ok(());
        }
    };

    // Only a real TLS ClientHello (SNI recovered) is intercepted for injection.
    if let (Some(injector), Some(name)) = (injection.as_ref(), sni.as_deref())
        && injector.should_inject(name)
    {
        tracing::info!("INJECT transparent {name}:{port}");
        return Box::pin(injector.handle(conn, buf, name, &addrs)).await;
    }

    let mut upstream =
        match tokio::time::timeout(Duration::from_secs(10), TcpStream::connect(addrs.as_slice()))
            .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                tracing::warn!("dial upstream {policy_target} failed: {e}");
                return Ok(());
            }
            Err(_) => {
                tracing::warn!("dial upstream {policy_target} timed out");
                return Ok(());
            }
        };

    // Replay the peeked bytes, then splice bidirectionally.
    if !buf.is_empty() {
        upstream.write_all(&buf).await?;
    }
    tokio::io::copy_bidirectional(&mut conn, &mut upstream).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tokio::io::AsyncReadExt;

    fn write_policy(json: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("policy.json");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(json.as_bytes()).unwrap();
        (dir, path)
    }

    /// Without an iptables REDIRECT there is no conntrack entry; the kernel
    /// may either error or report the socket's own local address. Either way
    /// the safe `nix` getsockopt path must run without panicking — that's what
    /// we assert here (full splice needs real iptables and can't run in CI).
    #[tokio::test]
    async fn original_dst_path_is_wired() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            // Just exercise the getsockopt — result shape doesn't matter on loopback.
            let _ = original_dst(&stream);
        });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"x").await.unwrap();
        server.await.unwrap();
        drop(client);
    }

    /// End-to-end-ish: the deny path closes the connection without forwarding.
    /// On loopback there is no original destination to recover, so the handler
    /// closes the connection — confirming it never panics and always closes.
    #[tokio::test]
    async fn deny_closes_connection() {
        let (_dir, path) = write_policy(r#"{"allowed_hosts": [], "default": "deny"}"#);
        let policy = Arc::new(PolicyManager::new(&path));
        policy.load().unwrap();

        let private = Arc::new(PrivateAllow::for_tests(false, ""));
        let listener =
            TransparentListener::bind("127.0.0.1:0", policy, None, private).await.unwrap();
        let addr = listener.listener.local_addr().unwrap();
        tokio::spawn(async move { listener.serve().await });

        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"GET / HTTP/1.1\r\nHost: evil.example.com\r\n\r\n").await.unwrap();
        // The server closes the connection (original_dst fails on loopback, or
        // policy denies): the read returns 0 bytes / EOF.
        let mut buf = [0u8; 16];
        let n = client.read(&mut buf).await.unwrap_or(0);
        assert_eq!(n, 0, "denied connection should be closed by the proxy");
    }
}
