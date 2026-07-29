//! Built-in deny set that OVERRIDES the allow-list, so a buggy or malicious
//! `policy.json` can never open these destinations to the worker.
//!
//! Denied:
//! - Link-local `169.254.0.0/16` / `fe80::/10` — cloud IMDS + EKS Pod Identity,
//!   the credential surface. NEVER relaxed.
//! - Loopback, RFC1918 + IPv6 ULA, CGNAT `100.64/10`, unspecified and IPv4
//!   broadcast. Relaxed by [`allow_private_ips_from_env`] (the dev
//!   docker-compose stack reaches its own server/registry over private IPs
//!   through this proxy); link-local stays denied even then.
//! - `metadata.google.internal` and any `GUARD_PROXY_DENY_HOSTS` extras.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::OnceLock;

/// Metadata/credential hostnames that are always denied. IP-literal forms are
/// caught by [`ip_is_denied`] / the IP branch of [`host_is_denied`].
const DENIED_HOSTNAMES: &[&str] = &[
    "metadata.google.internal", // GCP/GKE metadata server DNS name
    "metadata",                 // short GCP alias
];

/// The dev opt-out: `GUARD_PROXY_ALLOW_PRIVATE_IPS` truthy relaxes the private /
/// loopback / CGNAT denials (but NOT link-local) so the docker-compose dev stack
/// can reach its own server/registry over RFC1918 through the proxy. Read once by
/// the listener at bind time and threaded into every deny check, so the value is
/// fixed per process while the deny functions stay pure and testable.
#[must_use]
pub fn allow_private_ips_from_env() -> bool {
    std::env::var("GUARD_PROXY_ALLOW_PRIVATE_IPS").is_ok_and(|v| {
        matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
    })
}

/// True if `ip` must never be dialed. Link-local (`169.254/16` / `fe80::/10`) —
/// the cloud metadata + EKS Pod Identity surface — is always denied. Loopback,
/// RFC1918/ULA, CGNAT, unspecified and broadcast are also denied unless
/// `allow_private` (the dev opt-out) is set.
#[must_use]
pub fn ip_is_denied(ip: IpAddr, allow_private: bool) -> bool {
    // Fold IPv4-mapped IPv6 (`::ffff:a.b.c.d`) down to IPv4 so a mapped metadata
    // or private address can't slip past the v4 checks.
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        IpAddr::V4(_) => ip,
    };
    if is_link_local(ip) {
        return true; // metadata/credential surface — never opt-out-able
    }
    if allow_private {
        return false;
    }
    is_private_or_local(ip)
}

/// Link-local: IPv4 `169.254.0.0/16` (cloud IMDS + EKS Pod Identity) or IPv6
/// `fe80::/10`.
const fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Loopback / private / CGNAT / unspecified / broadcast — internal and host-local
/// destinations. Callers gate this behind the `allow_private` opt-out.
const fn is_private_or_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || is_cgnat(v4)
        }
        // 0xfc00 covers fc00::/7 (unique-local); loopback ::1 and unspecified ::
        // are their own checks. (v4-mapped was already folded out above.)
        IpAddr::V6(v6) => {
            v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00
        }
    }
}

/// CGNAT shared address space `100.64.0.0/10` (RFC 6598).
const fn is_cgnat(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && o[1] >= 64 && o[1] <= 127
}

/// True if `host` (a bare hostname or IP literal, no port) is on the built-in
/// deny set or the `GUARD_PROXY_DENY_HOSTS` extension. Matching is
/// case-insensitive; an IP-literal host is checked against [`ip_is_denied`] so
/// an SNI-less request straight to `169.254.169.254` (or a private IP) is caught.
#[must_use]
pub fn host_is_denied(host: &str, allow_private: bool) -> bool {
    host_is_denied_with(host, extra_denied_hosts(), allow_private)
}

fn host_is_denied_with(host: &str, extra: &[String], allow_private: bool) -> bool {
    let host = host.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    if let Ok(ip) = host.parse::<IpAddr>()
        && ip_is_denied(ip, allow_private)
    {
        return true;
    }
    let host = host.as_str();
    DENIED_HOSTNAMES.contains(&host) || extra.iter().any(|d| host == d.as_str())
}

/// Resolves `host_port` (`host:port`, DNS name or IP literal) IN THE PROXY and
/// returns every resolved socket address — but only if NONE of them is denied by
/// [`ip_is_denied`]. A name that resolves even partially into a denied range is
/// refused, which closes the forged-name / DNS-rebinding gap.
///
/// Callers MUST connect to one of the returned addresses and MUST NOT re-resolve
/// `host_port`, so the address that was checked is the one dialed (no TOCTOU
/// window between the denylist check and connect).
pub async fn resolve_allowed(
    host_port: &str,
    allow_private: bool,
) -> anyhow::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = tokio::net::lookup_host(host_port)
        .await
        .map_err(|e| anyhow::anyhow!("resolving {host_port}: {e}"))?
        .collect();
    anyhow::ensure!(!addrs.is_empty(), "no addresses resolved for {host_port}");
    if let Some(denied) = addrs.iter().find(|a| ip_is_denied(a.ip(), allow_private)) {
        anyhow::bail!("resolved address {denied} for {host_port} is in the built-in denylist");
    }
    Ok(addrs)
}

/// Parses `GUARD_PROXY_DENY_HOSTS` once, lower-cased. Absent/empty → no extras.
fn extra_denied_hosts() -> &'static [String] {
    static EXTRA: OnceLock<Vec<String>> = OnceLock::new();
    EXTRA.get_or_init(|| {
        std::env::var("GUARD_PROXY_DENY_HOSTS").map_or_else(
            |_| Vec::new(),
            |v| {
                v.split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .filter(|s| !s.is_empty())
                    .collect()
            },
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn link_local_denied_even_with_optout() {
        for s in ["169.254.169.254", "169.254.170.23", "169.254.0.1"] {
            assert!(ip_is_denied(ip(s), false), "{s} must be denied");
            assert!(ip_is_denied(ip(s), true), "{s} metadata range is never opt-out-able");
        }
    }

    #[test]
    fn private_and_local_ranges_denied_by_default() {
        for s in [
            "127.0.0.1",
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
        ] {
            assert!(ip_is_denied(ip(s), false), "{s} must be denied without the opt-out");
        }
    }

    #[test]
    fn public_ips_allowed() {
        for s in ["140.82.121.4", "1.1.1.1", "8.8.8.8"] {
            assert!(!ip_is_denied(ip(s), false), "{s} is public and must be allowed");
        }
    }

    #[test]
    fn dev_optout_allows_private_but_not_metadata() {
        for s in ["127.0.0.1", "10.0.0.1", "192.168.1.1", "100.64.0.1"] {
            assert!(!ip_is_denied(ip(s), true), "{s} allowed under the dev opt-out");
        }
        assert!(ip_is_denied(ip("169.254.169.254"), true), "metadata still denied under opt-out");
    }

    #[test]
    fn ipv6_local_ranges_denied() {
        for s in ["::1", "::", "fc00::1", "fe80::1"] {
            assert!(ip_is_denied(ip(s), false), "{s} must be denied");
        }
        assert!(!ip_is_denied(ip("2606:4700::1111"), false), "public v6 allowed");
    }

    #[test]
    fn ipv4_mapped_ipv6_folds_to_v4() {
        // A mapped metadata address must not slip past as "some v6".
        assert!(
            ip_is_denied(ip("::ffff:169.254.169.254"), true),
            "mapped metadata denied under opt-out"
        );
        assert!(ip_is_denied(ip("::ffff:10.0.0.1"), false), "mapped private denied by default");
        assert!(!ip_is_denied(ip("::ffff:140.82.121.4"), false), "mapped public allowed");
    }

    #[test]
    fn ip_literal_denied_via_host() {
        // No SNI recovered → the proxy checks the IP-literal host string.
        assert!(host_is_denied_with("169.254.169.254", &[], false));
        assert!(host_is_denied_with("169.254.170.23", &[], false));
        assert!(host_is_denied_with("10.0.0.1", &[], false));
        assert!(host_is_denied_with("127.0.0.1", &[], false));
        // Metadata stays denied under the opt-out; a private literal does not.
        assert!(host_is_denied_with("169.254.169.254", &[], true));
        assert!(!host_is_denied_with("10.0.0.1", &[], true));
    }

    #[test]
    fn metadata_hostnames_denied() {
        assert!(host_is_denied_with("metadata.google.internal", &[], false));
        assert!(host_is_denied_with("METADATA.GOOGLE.INTERNAL", &[], false)); // case-insensitive
    }

    #[test]
    fn normal_hosts_allowed() {
        assert!(!host_is_denied_with("api.github.com", &[], false));
        assert!(!host_is_denied_with("registry.npmjs.org", &[], false));
        assert!(!host_is_denied_with("140.82.121.4", &[], false));
    }

    #[test]
    fn extra_deny_covers_openbao() {
        let extra = vec!["openbao.security.svc.cluster.local".to_string()];
        assert!(host_is_denied_with("openbao.security.svc.cluster.local", &extra, false));
        assert!(host_is_denied_with("OpenBao.security.svc.cluster.local", &extra, false));
        // Still allowed when not configured as an extra deny.
        assert!(!host_is_denied_with("openbao.security.svc.cluster.local", &[], false));
    }

    #[tokio::test]
    async fn resolve_allowed_rejects_denied_literals() {
        // IP-literal authorities resolve without DNS, so this is offline.
        assert!(resolve_allowed("169.254.169.254:80", false).await.is_err());
        assert!(resolve_allowed("10.0.0.1:443", false).await.is_err());
        assert!(resolve_allowed("127.0.0.1:80", false).await.is_err());
        // Metadata stays denied even with the opt-out.
        assert!(resolve_allowed("169.254.169.254:80", true).await.is_err());
    }

    #[tokio::test]
    async fn resolve_allowed_accepts_public_literal() {
        let addrs = resolve_allowed("140.82.121.4:443", false).await.unwrap();
        assert_eq!(addrs, vec!["140.82.121.4:443".parse().unwrap()]);
    }

    #[tokio::test]
    async fn resolve_allowed_permits_private_under_optout() {
        let addrs = resolve_allowed("127.0.0.1:8080", true).await.unwrap();
        assert_eq!(addrs, vec!["127.0.0.1:8080".parse().unwrap()]);
    }
}
