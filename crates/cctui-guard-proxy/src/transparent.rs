//! Transparent proxy mode: the iptables REDIRECT target. Connections arrive as
//! raw bytes (a TLS `ClientHello` or a plaintext HTTP request), not as an HTTP
//! CONNECT. We recover the original destination via `SO_ORIGINAL_DST`, enforce
//! policy on the recovered SNI / Host name, then tunnel. Ported from the Go
//! reference `transparent.go`.

use std::net::{IpAddr, Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use nix::sys::socket::getsockopt;
use nix::sys::socket::sockopt::OriginalDst;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::denylist::{host_is_denied, host_private_allowed, ip_is_denied, resolve_allowed};
use crate::inject::Injector;
use crate::peek::{extract_http_host, extract_sni};
use crate::policy::PolicyManager;

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
    allow_private: bool,
    private_allowed: Arc<Vec<String>>,
}

impl TransparentListener {
    pub async fn bind(
        addr: &str,
        policy: Arc<PolicyManager>,
        injection: Option<Arc<Injector>>,
    ) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let allow_private = crate::denylist::allow_private_ips_from_env();
        let private_allowed = Arc::new(crate::denylist::private_allowed_hosts_from_env());
        Ok(Self { listener, policy, injection, allow_private, private_allowed })
    }

    pub async fn serve(self) -> anyhow::Result<()> {
        loop {
            let (stream, _peer) = self.listener.accept().await?;
            let policy = self.policy.clone();
            let injection = self.injection.clone();
            let allow_private = self.allow_private;
            let private_allowed = self.private_allowed.clone();
            tokio::spawn(async move {
                if let Err(e) =
                    handle_connection(stream, policy, injection, allow_private, private_allowed)
                        .await
                {
                    tracing::debug!("transparent connection ended: {e}");
                }
            });
        }
    }
}

// Linear name-recovery (SNI/Host) → fail-closed policy check → dial → splice
// flow; complexity is the per-step timeout/error branches, not nesting. Splitting
// would scatter the security-critical fail-closed control flow across helpers.
#[allow(clippy::cognitive_complexity)]
async fn handle_connection(
    mut conn: TcpStream,
    policy: Arc<PolicyManager>,
    injection: Option<Arc<Injector>>,
    allow_private: bool,
    private_allowed: Arc<Vec<String>>,
) -> anyhow::Result<()> {
    let dst = original_dst(&conn)?;
    let host_port = dst.to_string();
    let port = dst.port();

    // Peek the first bytes to recover the intended hostname. The policy is a
    // hostname allow-list, so matching on the SO_ORIGINAL_DST IP would never
    // hit — we must enforce against the name.
    let mut buf = vec![0u8; 4096];
    let n = match tokio::time::timeout(Duration::from_secs(2), conn.read(&mut buf)).await {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => 0, // read timed out: no name recovered, fall through to deny
    };

    let mut sni = None;
    let mut http_host = None;
    if n > 0 {
        sni = extract_sni(&buf[..n]);
        if sni.is_none() {
            http_host = extract_http_host(&buf[..n]);
        }
    }

    // Build the policy target from the recovered name, falling back to the
    // original-destination IP:port when no name is available (which then fails
    // a hostname allow-list — fail closed).
    let name = sni.as_deref().or(http_host.as_deref());
    let policy_target = name.map_or_else(|| host_port.clone(), |n| format!("{n}:{port}"));

    // Scoped exemptions require a recovered name — never the bare original-dst
    // IP — and the dial goes to the proxy's own resolution of that name.
    let allow_private =
        allow_private || name.is_some_and(|n| host_private_allowed(n, &private_allowed));

    // Built-in deny overrides the allow-list: match on the recovered name AND the
    // original-destination IP, so an IP-literal metadata request with no SNI is
    // still caught early.
    if ip_is_denied(IpAddr::V4(*dst.ip()), allow_private)
        || name.is_some_and(|n| host_is_denied(n, allow_private))
    {
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

    // TLS-terminating credential injection for allowlisted hosts (CCT-718). Only
    // a real TLS ClientHello (SNI recovered) is intercepted; everything else
    // keeps the passthrough splice below.
    if let (Some(injector), Some(name)) = (injection.as_ref(), sni.as_deref())
        && injector.should_inject(name)
    {
        tracing::info!("INJECT transparent {name}:{port}");
        return Box::pin(injector.handle(conn, buf[..n].to_vec(), name, port)).await;
    }

    // Resolve the allow-listed target IN THE PROXY and dial an address we just
    // validated against the denylist — NOT the worker-supplied SO_ORIGINAL_DST.
    // A forged name can then only reach the host it actually names, and a name
    // resolving into a denied range is refused.
    let addrs = match resolve_allowed(&policy_target, allow_private).await {
        Ok(addrs) => addrs,
        Err(e) => {
            tracing::warn!("resolve {policy_target} failed (orig={host_port}): {e}");
            policy.record(&policy_target, false, "resolve/denylist");
            return Ok(());
        }
    };
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
    if n > 0 {
        upstream.write_all(&buf[..n]).await?;
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

        let listener = TransparentListener::bind("127.0.0.1:0", policy, None).await.unwrap();
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
