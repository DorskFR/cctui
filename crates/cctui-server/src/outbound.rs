//! SSRF guard for server-initiated requests to user-supplied URLs.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, LazyLock, OnceLock, RwLock};

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

/// `CCTUI_UPSTREAM_ALLOWED_HOSTS`, as `host[:port]` entries.
pub fn env_upstream_entries() -> Vec<String> {
    std::env::var("CCTUI_UPSTREAM_ALLOWED_HOSTS")
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(str::to_owned)
        .collect()
}

/// The host and port of `CCTUI_CLAUDE_LITELLM_ENDPOINT`, which the managed shim
/// account points at; always allowed on top of the editable list.
pub fn managed_upstream_entries() -> Vec<String> {
    std::env::var("CCTUI_CLAUDE_LITELLM_ENDPOINT")
        .ok()
        .and_then(|e| reqwest::Url::parse(e.trim()).ok())
        .and_then(|url| {
            let host = url.host_str()?.to_owned();
            Some(
                url.port_or_known_default().map_or_else(|| host.clone(), |p| format!("{host}:{p}")),
            )
        })
        .into_iter()
        .collect()
}

fn build_upstream_allowlist(saved: &[String], env: &[String]) -> Vec<AllowedHost> {
    parse_allowlist(&[saved, env, &managed_upstream_entries()].concat().join(","))
}

static UPSTREAM_ALLOWED_HOSTS: LazyLock<RwLock<Arc<Vec<AllowedHost>>>> =
    LazyLock::new(|| RwLock::new(Arc::new(build_upstream_allowlist(&[], &env_upstream_entries()))));

/// Hosts per-account upstreams may reach regardless of the guard: the saved
/// list, the env seed and the managed endpoint, all at once.
pub fn upstream_allowlist() -> Arc<Vec<AllowedHost>> {
    UPSTREAM_ALLOWED_HOSTS.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
}

pub fn set_upstream_allowlist(saved: &[String]) {
    let next = Arc::new(build_upstream_allowlist(saved, &env_upstream_entries()));
    *UPSTREAM_ALLOWED_HOSTS.write().unwrap_or_else(std::sync::PoisonError::into_inner) = next;
}

pub fn no_allowlist() -> Arc<Vec<AllowedHost>> {
    static EMPTY: LazyLock<Arc<Vec<AllowedHost>>> = LazyLock::new(|| Arc::new(Vec::new()));
    EMPTY.clone()
}

fn valid_label_host(h: &str) -> bool {
    h.len() <= 253
        && h.trim_end_matches('.').split('.').all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        })
}

/// Checks one `host[:port]` entry and returns it normalized.
pub fn normalize_allowlist_entry(raw: &str) -> Result<String, String> {
    let entry = raw.trim();
    let err = || format!("`{entry}` is not a host or host:port");
    if entry.is_empty() || entry.contains("://") || entry.contains(['/', '*', ' ', ',', '@']) {
        return Err(err());
    }
    let (host, port) = if let Some(rest) = entry.strip_prefix('[') {
        let (h, tail) = rest.split_once(']').ok_or_else(err)?;
        h.parse::<Ipv6Addr>().map_err(|_| err())?;
        match tail {
            "" => (format!("[{}]", h.to_ascii_lowercase()), None),
            t => (
                format!("[{}]", h.to_ascii_lowercase()),
                Some(t.strip_prefix(':').ok_or_else(err)?),
            ),
        }
    } else if entry.parse::<Ipv6Addr>().is_ok() {
        (format!("[{}]", entry.to_ascii_lowercase()), None)
    } else {
        let (h, p) = match entry.rsplit_once(':') {
            Some((h, p)) => (h, Some(p)),
            None => (entry, None),
        };
        if h.parse::<Ipv4Addr>().is_err() && !valid_label_host(h) {
            return Err(err());
        }
        (normalize_host(h), p)
    };
    match port {
        None => Ok(host),
        Some(p) => match p.parse::<u16>() {
            Ok(n) if n > 0 => Ok(format!("{host}:{n}")),
            _ => Err(format!("`{entry}` has an invalid port")),
        },
    }
}

fn normalize_host(host: &str) -> String {
    bare_host(host).trim_end_matches('.').to_ascii_lowercase()
}

pub fn parse_allowlist(raw: &str) -> Vec<AllowedHost> {
    raw.split(',')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .map(|entry| {
            let split = entry.strip_prefix('[').map_or_else(
                || match entry.rsplit_once(':') {
                    Some((h, p)) if !h.contains(':') => Some((h, Some(p))),
                    _ => None,
                },
                |rest| rest.split_once(']').map(|(h, tail)| (h, tail.strip_prefix(':'))),
            );
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
    validate_outbound_url(raw, &upstream_allowlist()).await
}

/// The DNS-free part of [`validate_upstream_url`], for the request path; names
/// are re-checked at connect time by [`upstream_client`]'s resolver.
pub fn upstream_url_permitted(raw: &str) -> Result<(), OutboundUrlError> {
    precheck(raw, &upstream_allowlist()).map(|_| ())
}

/// Drops internal addresses from every resolution, so a name that passed
/// validation cannot later be rebound onto an internal address.
/// Ports are not visible here; [`precheck`] enforces them on every request.
struct GuardedResolver {
    allow: fn() -> Arc<Vec<AllowedHost>>,
}

impl reqwest::dns::Resolve for GuardedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        let open = host_allowlisted(&host, &(self.allow)());
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

/// A client that follows no redirects and resolves names through the guard.
pub fn guarded_client(allow: fn() -> Arc<Vec<AllowedHost>>) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(GuardedResolver { allow }))
        .build()
        .expect("build guarded client")
}

/// Client for user-supplied upstreams.
pub fn upstream_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| guarded_client(upstream_allowlist))
}

#[cfg(test)]
mod tests {
    use super::{
        OutboundUrlError, ip_is_internal, normalize_allowlist_entry, parse_allowlist,
        validate_outbound_url,
    };

    #[test]
    fn env_hosts_stay_allowed_alongside_saved_ones() {
        let allow = super::build_upstream_allowlist(
            &["saved.internal:8080".to_owned()],
            &["env.internal".to_owned()],
        );
        super::precheck("http://saved.internal:8080/v1", &allow).unwrap();
        super::precheck("http://env.internal:9000/v1", &allow).unwrap();
        let only_env = super::build_upstream_allowlist(&[], &["env.internal".to_owned()]);
        super::precheck("http://env.internal/v1", &only_env).unwrap();
        assert!(super::precheck("http://saved.internal:8080/v1", &only_env).is_err());
    }

    #[test]
    fn allowlist_entries_are_validated_and_normalized() {
        for (raw, want) in [
            (" Ollama.LLM.svc ", "ollama.llm.svc"),
            ("litellm.llm.svc:4000", "litellm.llm.svc:4000"),
            ("10.0.0.5", "10.0.0.5"),
            ("10.0.0.5:8080", "10.0.0.5:8080"),
            ("[FD00::1]:8080", "[fd00::1]:8080"),
            ("fd00::1", "[fd00::1]"),
            ("minio", "minio"),
        ] {
            assert_eq!(normalize_allowlist_entry(raw).unwrap(), want, "{raw}");
        }
        for bad in [
            "",
            "https://example.com",
            "example.com/v1",
            "*.example.com",
            "exa mple.com",
            "example.com:0",
            "example.com:99999",
            "example.com:abc",
            "-bad.example.com",
            "a..b",
            "[fd00::1",
            "user@host",
        ] {
            assert!(normalize_allowlist_entry(bad).is_err(), "{bad}");
        }
    }

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
