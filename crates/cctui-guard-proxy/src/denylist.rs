//! Built-in, un-allowlistable deny set for metadata/credential endpoints
//! (CCT-720).
//!
//! The whole secret-source model collapses if the worker container can reach the
//! credential backend directly. These endpoints are therefore refused even if a
//! buggy or malicious `policy.json` lists them — this check OVERRIDES the
//! allow-list. It is belt-and-suspenders with the iptables REJECT that
//! `worker-net-init.sh` installs for `169.254.0.0/16`.
//!
//! Coverage:
//! - `169.254.0.0/16` (link-local): AWS/GCP/Azure IMDS (`169.254.169.254`) and
//!   the EKS Pod Identity Agent (`169.254.170.23`) — the prod path's credential
//!   surface.
//! - GCP metadata's DNS name (`metadata.google.internal`).
//! - Extra hostnames from `GUARD_PROXY_DENY_HOSTS` (comma-separated) — in dev
//!   this carries the OpenBao/Vault host so the worker can never reach it even
//!   if policy is buggy. The sidecar reaches `OpenBao` directly (not via this
//!   proxy), so denying it here only affects the worker egress path.

use std::net::Ipv4Addr;
use std::sync::OnceLock;

/// Metadata/credential hostnames that are always denied. IP-literal forms are
/// caught by [`ip_is_denied`] / the IP branch of [`host_is_denied`].
const DENIED_HOSTNAMES: &[&str] = &[
    "metadata.google.internal",  // GCP/GKE metadata server DNS name
    "metadata",                  // short GCP alias
];

/// True if `ip` is in the link-local metadata range (`169.254.0.0/16`), which
/// covers cloud IMDS and the EKS Pod Identity Agent.
#[must_use]
pub const fn ip_is_denied(ip: Ipv4Addr) -> bool {
    ip.is_link_local()
}

/// True if `host` (a bare hostname or IP literal, no port) is on the built-in
/// deny set or the `GUARD_PROXY_DENY_HOSTS` extension. Matching is
/// case-insensitive; an IP-literal host is checked against [`ip_is_denied`] so
/// an SNI-less request straight to `169.254.169.254` is caught.
#[must_use]
pub fn host_is_denied(host: &str) -> bool {
    host_is_denied_with(host, extra_denied_hosts())
}

fn host_is_denied_with(host: &str, extra: &[String]) -> bool {
    let host = host.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
    if let Ok(ip) = host.parse::<Ipv4Addr>()
        && ip_is_denied(ip)
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

    #[test]
    fn link_local_ips_denied() {
        assert!(ip_is_denied("169.254.169.254".parse().unwrap())); // AWS/GCP/Azure IMDS
        assert!(ip_is_denied("169.254.170.23".parse().unwrap())); // EKS Pod Identity Agent
        assert!(ip_is_denied("169.254.0.1".parse().unwrap())); // anywhere in /16
    }

    #[test]
    fn normal_ips_allowed() {
        assert!(!ip_is_denied("140.82.121.4".parse().unwrap())); // github
        assert!(!ip_is_denied("10.0.0.1".parse().unwrap()));
        assert!(!ip_is_denied("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn ip_literal_metadata_denied_via_host() {
        // No SNI recovered → the proxy checks the IP-literal host string.
        assert!(host_is_denied_with("169.254.169.254", &[]));
        assert!(host_is_denied_with("169.254.170.23", &[]));
    }

    #[test]
    fn metadata_hostnames_denied() {
        assert!(host_is_denied_with("metadata.google.internal", &[]));
        assert!(host_is_denied_with("METADATA.GOOGLE.INTERNAL", &[])); // case-insensitive
    }

    #[test]
    fn normal_hosts_allowed() {
        assert!(!host_is_denied_with("api.github.com", &[]));
        assert!(!host_is_denied_with("registry.npmjs.org", &[]));
        assert!(!host_is_denied_with("140.82.121.4", &[]));
    }

    #[test]
    fn extra_deny_covers_openbao() {
        let extra = vec!["openbao.security.svc.cluster.local".to_string()];
        assert!(host_is_denied_with("openbao.security.svc.cluster.local", &extra));
        assert!(host_is_denied_with("OpenBao.security.svc.cluster.local", &extra));
        // Still allowed when not configured as an extra deny.
        assert!(!host_is_denied_with("openbao.security.svc.cluster.local", &[]));
    }
}
