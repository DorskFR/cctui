//! SSRF guard for server-initiated requests to user-supplied URLs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, LazyLock, OnceLock};

#[derive(Debug)]
pub enum OutboundUrlError {
    Malformed,
    NotHttps,
    NoHost,
    Unresolvable,
    Internal,
}

impl std::fmt::Display for OutboundUrlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Malformed => "must be a valid absolute URL",
            Self::NotHttps => "must use the https scheme",
            Self::NoHost => "must include a host",
            Self::Unresolvable => "host does not resolve",
            Self::Internal => "resolves to a private or loopback address",
        })
    }
}

fn ipv4_is_internal(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        // CGNAT 100.64.0.0/10; `Ipv4Addr::is_shared` is still unstable.
        || (a == 100 && (64..=127).contains(&b))
}

fn ipv6_is_internal(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    // `to_ipv4` also maps `::`/`::1`, but those return above, so any remaining
    // embedded IPv4 (v4-mapped or deprecated v4-compatible) is a real target.
    if let Some(v4) = ip.to_ipv4() {
        return ipv4_is_internal(v4);
    }
    let seg0 = ip.segments()[0];
    (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80
}

pub fn ip_is_internal(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_is_internal(v4),
        IpAddr::V6(v6) => ipv6_is_internal(v6),
    }
}

/// Names that only mean something inside the cluster or host: single-label
/// names resolve through the pod's search domains.
fn host_is_cluster_local(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    !h.contains('.')
        || [".localhost", ".local", ".internal", ".svc", ".cluster.local"]
            .iter()
            .any(|s| h.ends_with(s))
}

fn bare_host(host: &str) -> &str {
    host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host)
}

/// An allowlist entry; without a port it allows every port on the host.
#[derive(Debug, PartialEq, Eq)]
pub struct AllowedHost {
    host: String,
    port: Option<u16>,
}

/// Hosts an operator lets per-account upstreams reach regardless of the guard:
/// `CCTUI_UPSTREAM_ALLOWED_HOSTS` (comma-separated `host[:port]`) plus the host
/// and port of `CCTUI_CLAUDE_LITELLM_ENDPOINT`, which the managed shim account
/// points at.
pub static UPSTREAM_ALLOWED_HOSTS: LazyLock<Vec<AllowedHost>> = LazyLock::new(|| {
    let mut hosts =
        parse_allowlist(&std::env::var("CCTUI_UPSTREAM_ALLOWED_HOSTS").unwrap_or_default());
    if let Some(url) = std::env::var("CCTUI_CLAUDE_LITELLM_ENDPOINT")
        .ok()
        .and_then(|e| reqwest::Url::parse(e.trim()).ok())
        && let Some(host) = url.host_str()
    {
        hosts.push(AllowedHost { host: normalize_host(host), port: url.port_or_known_default() });
    }
    hosts
});

fn normalize_host(host: &str) -> String {
    bare_host(host).trim_end_matches('.').to_ascii_lowercase()
}

pub fn parse_allowlist(raw: &str) -> Vec<AllowedHost> {
    raw.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|entry| {
            let split = if let Some(rest) = entry.strip_prefix('[') {
                rest.split_once(']').map(|(h, tail)| (h, tail.strip_prefix(':')))
            } else {
                match entry.rsplit_once(':') {
                    Some((h, p)) if !h.contains(':') => Some((h, Some(p))),
                    _ => None,
                }
            };
            match split {
                Some((h, Some(p))) => {
                    AllowedHost { host: normalize_host(h), port: p.parse().ok().or(Some(0)) }
                }
                Some((h, None)) => AllowedHost { host: normalize_host(h), port: None },
                None => AllowedHost { host: normalize_host(entry), port: None },
            }
        })
        .collect()
}

fn host_allowlisted(host: &str, allow: &[AllowedHost]) -> bool {
    let h = normalize_host(host);
    allow.iter().any(|a| a.host == h)
}

fn allowlisted(url: &reqwest::Url, allow: &[AllowedHost]) -> bool {
    let Some(host) = url.host_str() else { return false };
    let h = normalize_host(host);
    let port = url.port_or_known_default();
    allow.iter().any(|a| a.host == h && a.port.is_none_or(|p| Some(p) == port))
}

/// DNS-free checks; `Ok(Some(url))` means the host name still needs resolving.
fn precheck(raw: &str, allow: &[AllowedHost]) -> Result<Option<reqwest::Url>, OutboundUrlError> {
    let url = reqwest::Url::parse(raw).map_err(|_| OutboundUrlError::Malformed)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(OutboundUrlError::NotHttps);
    }
    let host = url.host_str().ok_or(OutboundUrlError::NoHost)?;
    if allowlisted(&url, allow) {
        return Ok(None);
    }
    if url.scheme() != "https" {
        return Err(OutboundUrlError::NotHttps);
    }
    if let Ok(ip) = bare_host(host).parse::<IpAddr>() {
        return if ip_is_internal(ip) { Err(OutboundUrlError::Internal) } else { Ok(None) };
    }
    if host_is_cluster_local(host) {
        return Err(OutboundUrlError::Internal);
    }
    Ok(Some(url))
}

/// Fail-closed: requires `https` (unless allowlisted) and refuses a host that
/// is cluster-local, resolves to an internal address, or does not resolve.
pub async fn validate_outbound_url(
    raw: &str,
    allow: &[AllowedHost],
) -> Result<(), OutboundUrlError> {
    let Some(url) = precheck(raw, allow)? else { return Ok(()) };
    let host = url.host_str().ok_or(OutboundUrlError::NoHost)?;
    let port = url.port_or_known_default().unwrap_or(443);
    let mut addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|_| OutboundUrlError::Unresolvable)?
        .peekable();
    if addrs.peek().is_none() {
        return Err(OutboundUrlError::Unresolvable);
    }
    for addr in addrs {
        if ip_is_internal(addr.ip()) {
            return Err(OutboundUrlError::Internal);
        }
    }
    Ok(())
}

/// A per-account upstream `base_url`, validated against the operator allowlist.
pub async fn validate_upstream_url(raw: &str) -> Result<(), OutboundUrlError> {
    validate_outbound_url(raw, &UPSTREAM_ALLOWED_HOSTS).await
}

/// The DNS-free part of [`validate_upstream_url`], for the request path; names
/// are re-checked at connect time by [`upstream_client`]'s resolver.
pub fn upstream_url_permitted(raw: &str) -> Result<(), OutboundUrlError> {
    precheck(raw, &UPSTREAM_ALLOWED_HOSTS).map(|_| ())
}

/// Drops internal addresses from every resolution, so a name that passed
/// validation cannot later be rebound onto an internal address.
/// Ports are not visible here; [`precheck`] enforces them on every request.
struct GuardedResolver {
    allow: &'static [AllowedHost],
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let open = host_allowlisted(&host, self.allow);
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| open || !ip_is_internal(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(format!("{host}: no permitted address").into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Client for user-supplied upstreams: no redirects, guarded DNS.
pub fn upstream_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .dns_resolver(Arc::new(GuardedResolver { allow: &UPSTREAM_ALLOWED_HOSTS }))
            .build()
            .expect("build upstream client")
    })
}

#[cfg(test)]
mod tests {
    use super::{OutboundUrlError, ip_is_internal, parse_allowlist, validate_outbound_url};

    #[test]
    fn ip_classifier_flags_internal_and_passes_public() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.31.0.1",
            "192.168.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:169.254.169.254",
        ] {
            assert!(ip_is_internal(ip.parse().unwrap()), "{ip} must be internal");
        }
        for ip in ["1.1.1.1", "8.8.8.8", "93.184.216.34", "2606:4700:4700::1111"] {
            assert!(!ip_is_internal(ip.parse().unwrap()), "{ip} must be public");
        }
    }

    #[tokio::test]
    async fn loopback_private_link_local_and_cluster_names_are_rejected() {
        for u in [
            "https://127.0.0.1:8080",
            "https://10.0.0.5/v1",
            "https://192.168.1.1",
            "https://169.254.169.254/latest/meta-data/",
            "https://[::1]/",
            "https://localhost:9000",
            "https://minio",
            "https://minio.storage.svc:9000",
            "https://minio.storage.svc.cluster.local",
            "https://metadata.google.internal",
        ] {
            assert!(
                matches!(validate_outbound_url(u, &[]).await, Err(OutboundUrlError::Internal)),
                "{u}"
            );
        }
        assert!(matches!(
            validate_outbound_url("http://1.1.1.1", &[]).await,
            Err(OutboundUrlError::NotHttps)
        ));
    }

    #[tokio::test]
    async fn an_allowlisted_host_bypasses_the_guard() {
        let allow = parse_allowlist(" Ollama.LLM.svc , 10.0.0.5 ,");
        validate_outbound_url("http://ollama.llm.svc:11434", &allow).await.unwrap();
        validate_outbound_url("http://10.0.0.5/v1", &allow).await.unwrap();
        assert!(validate_outbound_url("https://10.0.0.6/v1", &allow).await.is_err());
        assert!(validate_outbound_url("file://10.0.0.5/x", &allow).await.is_err());
    }

    #[tokio::test]
    async fn an_allowlisted_port_restricts_the_host_to_it() {
        let allow = parse_allowlist("litellm.llm.svc:4000,[fd00::1]:8080,10.0.0.7");
        validate_outbound_url("http://litellm.llm.svc:4000/v1", &allow).await.unwrap();
        assert!(validate_outbound_url("http://litellm.llm.svc:9000/v1", &allow).await.is_err());
        assert!(validate_outbound_url("http://litellm.llm.svc/v1", &allow).await.is_err());
        validate_outbound_url("http://[fd00::1]:8080/", &allow).await.unwrap();
        assert!(validate_outbound_url("http://[fd00::1]:8081/", &allow).await.is_err());
        validate_outbound_url("http://10.0.0.7:1234/", &allow).await.unwrap();
    }

    #[tokio::test]
    async fn public_https_is_accepted() {
        validate_outbound_url("https://1.1.1.1/v1", &[]).await.unwrap();
    }

    #[tokio::test]
    async fn the_upstream_client_does_not_follow_redirects() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 302 Found\r\nLocation: http://169.254.169.254/\r\n\
                          Content-Length: 0\r\nConnection: close\r\n\r\n",
                    )
                    .await;
            }
        });
        let resp =
            super::upstream_client().get(format!("http://{addr}/v1/models")).send().await.unwrap();
        assert_eq!(resp.status(), 302);
    }
}
