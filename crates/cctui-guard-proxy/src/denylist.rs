//! Built-in deny set that OVERRIDES the allow-list, so a buggy or malicious
//! `policy.json` can never open these destinations to the worker.
//!
//! Denied:
//! - Link-local `169.254.0.0/16` / `fe80::/10` and AWS's IPv6 IMDS / Pod
//!   Identity range `fd00:ec2::/32`: the credential surface. NEVER relaxed.
//! - IPv6 that carries an IPv4 target (v4-mapped, v4-compatible, NAT64
//!   `64:ff9b::/96`, 6to4 `2002::/16`) is classified as that IPv4.
//! - Loopback, RFC1918 + IPv6 ULA, CGNAT `100.64/10`, unspecified and IPv4
//!   broadcast. Relaxed per host by `private_allow`, or wholesale by the legacy
//!   [`allow_private_ips_from_env`]; link-local stays denied even then.
//! - `metadata.google.internal` and any `GUARD_PROXY_DENY_HOSTS` extras.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::OnceLock;

/// Metadata/credential hostnames that are always denied. IP-literal forms are
/// caught by [`ip_is_denied`] / the IP branch of [`host_is_denied`].
const DENIED_HOSTNAMES: &[&str] = &[
    "metadata.google.internal", // GCP/GKE metadata server DNS name
    "metadata",                 // short GCP alias
];

/// The legacy blanket opt-out: `GUARD_PROXY_ALLOW_PRIVATE_IPS` truthy relaxes the
/// private / loopback / CGNAT denials (but NOT link-local) for every host.
#[must_use]
pub fn allow_private_ips_from_env() -> bool {
    std::env::var("GUARD_PROXY_ALLOW_PRIVATE_IPS").is_ok_and(|v| {
        matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
    })
}

/// True if `ip` must never be dialed. The credential surface (link-local and
/// `fd00:ec2::/32`) is always denied. Loopback, RFC1918/ULA, CGNAT, unspecified
/// and broadcast are also denied unless `allow_private` (the dev opt-out) is set.
#[must_use]
pub fn ip_is_denied(ip: IpAddr, allow_private: bool) -> bool {
    let ip = canonical(ip);
    if is_credential_surface(ip) {
        return true;
    }
    if allow_private {
        return false;
    }
    is_private_or_local(ip)
}

/// The IPv4 target an IPv6 address stands for, if any, so a translated
/// metadata or private address can't slip past the v4 checks.
fn canonical(ip: IpAddr) -> IpAddr {
    let IpAddr::V6(v6) = ip else { return ip };
    if v6.is_loopback() || v6.is_unspecified() {
        return ip;
    }
    v6.to_ipv4().or_else(|| embedded_ipv4(v6)).map_or(ip, IpAddr::V4)
}

/// IPv4 embedded by NAT64's well-known `64:ff9b::/96` or 6to4's `2002::/16`.
fn embedded_ipv4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let seg = ip.segments();
    let from = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    if seg[0] == 0x0064 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
        return Some(from(seg[6], seg[7]));
    }
    if seg[0] == 0x2002 {
        return Some(from(seg[1], seg[2]));
    }
    None
}

/// Cloud IMDS + EKS Pod Identity: IPv4 `169.254.0.0/16`, IPv6 `fe80::/10` and
/// AWS's IPv6 endpoints in `fd00:ec2::/32`.
const fn is_credential_surface(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => {
            let seg = v6.segments();
            (seg[0] & 0xffc0) == 0xfe80 || (seg[0] == 0xfd00 && seg[1] == 0x0ec2)
        }
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
        // fc00::/7 unique-local.
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
    fn translated_ipv4_classified_as_embedded() {
        for s in ["64:ff9b::a9fe:a9fe", "2002:a9fe:a9fe::", "::169.254.169.254"] {
            assert!(ip_is_denied(ip(s), true), "{s} embeds metadata; never opt-out-able");
        }
        for s in ["64:ff9b::a00:5", "64:ff9b::7f00:1", "2002:c0a8:101::1"] {
            assert!(ip_is_denied(ip(s), false), "{s} embeds a private v4");
            assert!(!ip_is_denied(ip(s), true), "{s} relaxed by the opt-out");
        }
        assert!(!ip_is_denied(ip("64:ff9b::8c52:7904"), false), "NAT64 of a public v4 allowed");
        assert!(!ip_is_denied(ip("2002:808:808::"), false), "6to4 of a public v4 allowed");
        assert!(ip_is_denied(ip("::1"), false), "v6 loopback stays loopback");
    }

    #[test]
    fn aws_ipv6_metadata_never_relaxed() {
        for s in ["fd00:ec2::254", "fd00:ec2::23", "fd00:ec2::1"] {
            assert!(ip_is_denied(ip(s), false), "{s} must be denied");
            assert!(ip_is_denied(ip(s), true), "{s} is the credential surface");
            assert!(host_is_denied_with(&format!("[{s}]"), &[], true));
        }
        assert!(!ip_is_denied(ip("fd00:ec3::1"), true), "other ULA relaxed by the opt-out");
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
}
