//! A private destination is dialed only for a listed host, and only at an
//! address that host resolves to in the proxy, so a forged SNI/Host cannot ride
//! the exemption onto another private IP. The seeded list file exists because a
//! sidecar cannot read the worker's env.

use std::collections::HashSet;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Mutex, PoisonError, RwLock};
use std::time::SystemTime;

use crate::denylist::ip_is_denied;

const CONTROL_PLANE_URLS: &[&str] = &[
    "CCTUI_URL",
    "CCTUI_SERVER_URL",
    "CCTUI_BASE_URL",
    "REPLY_URL",
    "ANTHROPIC_BASE_URL",
    "OPENAI_BASE_URL",
    "FIREWORKS_BASE_URL",
    "GOOGLE_GEMINI_BASE_URL",
];

const LEGACY_WARNING: &str = "GUARD_PROXY_ALLOW_PRIVATE_IPS is set: every private destination is \
     reachable (only link-local stays denied); list the hosts in GUARD_PROXY_PRIVATE_ALLOW instead";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    host: String,
    /// `None` matches any port.
    port: Option<u16>,
    source: String,
}

impl Entry {
    fn matches(&self, host: &str, port: u16) -> bool {
        self.host == host && self.port.is_none_or(|p| p == port)
    }

    fn describe(&self) -> String {
        self.port.map_or_else(
            || format!("{}:* ({})", self.host, self.source),
            |p| format!("{}:{p} ({})", self.host, self.source),
        )
    }
}

#[derive(Default)]
struct Seeded {
    mtime: Option<SystemTime>,
    entries: Vec<Entry>,
}

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    Public,
    Allowed(String),
    Denied(String),
}

pub struct PrivateAllow {
    legacy: bool,
    fixed: Vec<Entry>,
    file: Option<PathBuf>,
    seeded: RwLock<Seeded>,
    logged: Mutex<HashSet<String>>,
}

impl PrivateAllow {
    /// Builds the list from the process env and logs what it allows.
    #[must_use]
    pub fn from_env(file: Option<PathBuf>, inject_hosts: &[String]) -> Self {
        let this = Self::build(
            crate::denylist::allow_private_ips_from_env(),
            |k| std::env::var(k).ok(),
            inject_hosts,
            file,
        );
        if let Some(warning) = this.legacy_warning() {
            tracing::warn!("{warning}");
        } else {
            tracing::info!("private egress limited to: {}", describe(&this.fixed));
        }
        this
    }

    fn build(
        legacy: bool,
        env: impl Fn(&str) -> Option<String>,
        inject_hosts: &[String],
        file: Option<PathBuf>,
    ) -> Self {
        let mut fixed: Vec<Entry> =
            CONTROL_PLANE_URLS.iter().filter_map(|k| url_entry(&env(k)?, k)).collect();
        if let Some(list) = env("GUARD_PROXY_PRIVATE_ALLOW") {
            fixed.extend(
                list.split(',').filter_map(|e| parse_entry(e, "GUARD_PROXY_PRIVATE_ALLOW")),
            );
        }
        fixed.extend(inject_hosts.iter().filter_map(|h| parse_entry(h, "injection rule")));
        Self {
            legacy,
            fixed,
            file,
            seeded: RwLock::new(Seeded::default()),
            logged: Mutex::new(HashSet::new()),
        }
    }

    #[cfg(test)]
    #[must_use]
    pub fn for_tests(legacy: bool, private_allow: &str) -> Self {
        let list = private_allow.to_owned();
        Self::build(legacy, |k| (k == "GUARD_PROXY_PRIVATE_ALLOW").then(|| list.clone()), &[], None)
    }

    #[must_use]
    pub const fn legacy_warning(&self) -> Option<&'static str> {
        if self.legacy { Some(LEGACY_WARNING) } else { None }
    }

    /// Name-level gate applied before the policy check: whether `host:port` may
    /// reach a private address at all. [`Self::resolve`] still binds the IP.
    #[must_use]
    pub fn exempts(&self, host: &str, port: u16) -> bool {
        self.legacy || self.rule_for(host, port).is_some()
    }

    /// Resolves `host:port` in the proxy and returns the addresses to dial, or
    /// refuses. Callers MUST dial only the returned addresses (never re-resolve),
    /// so the checked address is the dialed one. `orig_dst` is the transparent
    /// mode's `SO_ORIGINAL_DST`: a private one must be among the host's own
    /// addresses. Each private decision is logged once per host and reason.
    pub async fn resolve(
        &self,
        host: &str,
        port: u16,
        orig_dst: Option<IpAddr>,
    ) -> anyhow::Result<Vec<SocketAddr>> {
        let host_port = join_host_port(host, port);
        let mut addrs: Vec<SocketAddr> = tokio::net::lookup_host(&host_port)
            .await
            .map_err(|e| anyhow::anyhow!("resolving {host_port}: {e}"))?
            .collect();
        anyhow::ensure!(!addrs.is_empty(), "no addresses resolved for {host_port}");
        dial_order(&mut addrs, orig_dst);
        match self.decide(host, port, &addrs, orig_dst) {
            Decision::Public => Ok(addrs),
            Decision::Allowed(rule) => {
                self.log_once(&host_port, "ALLOW", &rule);
                Ok(addrs)
            }
            Decision::Denied(reason) => {
                self.log_once(&host_port, "DENY", &reason);
                anyhow::bail!("{host_port}: {reason}")
            }
        }
    }

    fn decide(
        &self,
        host: &str,
        port: u16,
        addrs: &[SocketAddr],
        orig_dst: Option<IpAddr>,
    ) -> Decision {
        if let Some(a) = addrs.iter().find(|a| ip_is_denied(a.ip(), true)) {
            return Decision::Denied(format!("resolves to {} in the built-in denylist", a.ip()));
        }
        let dst_private = orig_dst.is_some_and(is_private);
        if !dst_private && !addrs.iter().any(|a| is_private(a.ip())) {
            return Decision::Public;
        }
        if self.legacy {
            return Decision::Allowed("GUARD_PROXY_ALLOW_PRIVATE_IPS".to_owned());
        }
        let Some(rule) = self.rule_for(host, port) else {
            return Decision::Denied(
                "private address; host not on the private allow-list".to_owned(),
            );
        };
        if let Some(dst) = orig_dst.filter(|d| dst_private && !addrs.iter().any(|a| a.ip() == *d)) {
            return Decision::Denied(format!(
                "original destination {dst} is not an address of the host ({rule})"
            ));
        }
        Decision::Allowed(rule)
    }

    fn rule_for(&self, host: &str, port: u16) -> Option<String> {
        let host = host.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
        self.refresh();
        self.fixed
            .iter()
            .chain(self.seeded.read().unwrap_or_else(PoisonError::into_inner).entries.iter())
            .find(|e| e.matches(&host, port))
            .map(|e| e.source.clone())
    }

    /// Reloads the seeded list file when its mtime changes; a missing file is an
    /// empty list.
    fn refresh(&self) {
        let Some(path) = &self.file else { return };
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if self.seeded.read().unwrap_or_else(PoisonError::into_inner).mtime == mtime {
            return;
        }
        let entries = std::fs::read_to_string(path)
            .map(|text| parse_list(&text, "control-plane list"))
            .unwrap_or_default();
        tracing::info!("private allow-list {} loaded: {}", path.display(), describe(&entries));
        *self.seeded.write().unwrap_or_else(PoisonError::into_inner) = Seeded { mtime, entries };
    }

    fn log_once(&self, host_port: &str, verdict: &str, reason: &str) {
        let key = format!("{verdict} {host_port} {reason}");
        if !self.logged.lock().unwrap_or_else(PoisonError::into_inner).insert(key) {
            return;
        }
        if verdict == "ALLOW" {
            tracing::info!("private egress ALLOW {host_port} (rule: {reason})");
        } else {
            tracing::warn!("private egress DENY {host_port}: {reason}");
        }
    }
}

fn is_private(ip: IpAddr) -> bool {
    ip_is_denied(ip, false)
}

fn describe(entries: &[Entry]) -> String {
    if entries.is_empty() {
        return "(none)".to_owned();
    }
    entries.iter().map(Entry::describe).collect::<Vec<_>>().join(", ")
}

fn join_host_port(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Addresses are dialed in turn under one timeout, and the worker pod REJECTs
/// IPv6 egress (each attempt costs ~1s), so a dual-stack host listing its AAAA
/// records first would exhaust the budget before any IPv4 address. Dial the
/// worker's own original destination first, then IPv4, then IPv6.
fn dial_order(addrs: &mut [SocketAddr], orig_dst: Option<IpAddr>) {
    addrs.sort_by_key(|a| (Some(a.ip()) != orig_dst, a.is_ipv6()));
}

/// Parses `host`, `host:port`, `host:*`, `[v6]:port` or a bare v6 literal.
fn parse_entry(raw: &str, source: &str) -> Option<Entry> {
    let raw = raw.trim().to_ascii_lowercase();
    let (host, port) = if let Some(rest) = raw.strip_prefix('[') {
        let (host, tail) = rest.split_once(']')?;
        (host, tail.strip_prefix(':'))
    } else if raw.matches(':').count() > 1 {
        (raw.as_str(), None)
    } else {
        raw.split_once(':').map_or((raw.as_str(), None), |(h, p)| (h, Some(p)))
    };
    let port = match port {
        None | Some("*") => None,
        Some(p) => Some(p.parse::<u16>().ok()?),
    };
    if host.is_empty() {
        return None;
    }
    Some(Entry { host: host.to_owned(), port, source: source.to_owned() })
}

/// The `host:port` of a URL, defaulting the port from an http(s)/ws(s) scheme.
fn url_entry(url: &str, source: &str) -> Option<Entry> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit_once('@').map_or(authority, |(_, a)| a);
    let mut entry = parse_entry(authority, source)?;
    if entry.port.is_none() {
        entry.port = match scheme.to_ascii_lowercase().as_str() {
            "https" | "wss" => Some(443),
            "http" | "ws" => Some(80),
            _ => None,
        };
    }
    Some(entry)
}

/// One `host[:port] [label]` per line; blank lines and `#` comments skipped.
fn parse_list(text: &str, source: &str) -> Vec<Entry> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (host_port, label) =
                l.split_once(char::is_whitespace).map_or((l, ""), |(h, rest)| (h, rest.trim()));
            if label.is_empty() {
                parse_entry(host_port, source)
            } else {
                parse_entry(host_port, &format!("{source}: {label}"))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLUSTER_IP: &str = "10.43.12.7";
    const GATEWAY_LAN_IP: &str = "192.168.11.80";

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    fn addrs(ips: &[&str], port: u16) -> Vec<SocketAddr> {
        ips.iter().map(|s| SocketAddr::new(ip(s), port)).collect()
    }

    fn allow(legacy: bool, env: &[(&str, &str)]) -> PrivateAllow {
        let env: Vec<(String, String)> =
            env.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
        PrivateAllow::build(
            legacy,
            |k| env.iter().find(|(name, _)| name == k).map(|(_, v)| v.clone()),
            &[],
            None,
        )
    }

    fn control_plane() -> PrivateAllow {
        allow(
            false,
            &[
                ("CCTUI_SERVER_URL", "http://cctui.dev.svc.cluster.local:8080"),
                ("ANTHROPIC_BASE_URL", "https://cctui.dorsk.dev/api/gateway/anthropic"),
                ("REPLY_URL", "https://n8n.internal.example/webhook/abc?x=1"),
            ],
        )
    }

    #[test]
    fn control_plane_host_on_its_resolved_private_ip_allowed() {
        let pa = control_plane();
        let server = addrs(&[CLUSTER_IP], 8080);
        assert_eq!(
            pa.decide("cctui.dev.svc.cluster.local", 8080, &server, Some(ip(CLUSTER_IP))),
            Decision::Allowed("CCTUI_SERVER_URL".to_owned())
        );
        let gateway = addrs(&[GATEWAY_LAN_IP], 443);
        assert_eq!(
            pa.decide("cctui.dorsk.dev", 443, &gateway, None),
            Decision::Allowed("ANTHROPIC_BASE_URL".to_owned())
        );
        assert_eq!(
            pa.decide("N8N.internal.example", 443, &addrs(&["10.0.5.5"], 443), None),
            Decision::Allowed("REPLY_URL".to_owned())
        );
    }

    #[test]
    fn same_host_on_a_different_private_ip_denied() {
        let pa = control_plane();
        let server = addrs(&[CLUSTER_IP], 8080);
        let d = pa.decide("cctui.dev.svc.cluster.local", 8080, &server, Some(ip("10.43.0.1")));
        assert!(matches!(d, Decision::Denied(ref r) if r.contains("10.43.0.1")), "{d:?}");
    }

    #[test]
    fn listed_host_on_another_port_denied() {
        let pa = control_plane();
        let d = pa.decide("cctui.dev.svc.cluster.local", 5432, &addrs(&[CLUSTER_IP], 5432), None);
        assert!(matches!(d, Decision::Denied(_)), "{d:?}");
    }

    #[test]
    fn unlisted_private_host_denied() {
        let pa = control_plane();
        let d = pa.decide("postgres.internal", 5432, &addrs(&["10.0.0.9"], 5432), None);
        assert!(matches!(d, Decision::Denied(_)), "{d:?}");
    }

    #[test]
    fn unlisted_host_spoofed_onto_an_allowed_ip_denied() {
        let pa = control_plane();
        let spoofed = addrs(&["93.184.216.34"], 8080);
        let d = pa.decide("evil.example.com", 8080, &spoofed, Some(ip(CLUSTER_IP)));
        assert!(matches!(d, Decision::Denied(_)), "{d:?}");
    }

    #[test]
    fn explicit_private_allow_entries() {
        let list = " git.lan:22 , registry.lan ,10.9.9.9:443";
        let pa = allow(false, &[("GUARD_PROXY_PRIVATE_ALLOW", list)]);
        let git = |port| pa.decide("git.lan", port, &addrs(&["10.1.1.1"], port), None);
        assert!(matches!(git(22), Decision::Allowed(_)));
        assert!(matches!(git(443), Decision::Denied(_)));
        assert!(matches!(
            pa.decide("registry.lan", 5000, &addrs(&["10.1.1.2"], 5000), None),
            Decision::Allowed(_)
        ));
        assert!(matches!(
            pa.decide("10.9.9.9", 443, &addrs(&["10.9.9.9"], 443), Some(ip("10.9.9.9"))),
            Decision::Allowed(_)
        ));
        assert!(pa.exempts("git.lan", 22));
        assert!(!pa.exempts("git.lan", 23));
    }

    #[test]
    fn injection_hosts_are_listed_on_any_port() {
        let pa = PrivateAllow::build(
            false,
            |_| None,
            &["secretsmanager.ap-northeast-1.amazonaws.com".to_owned()],
            None,
        );
        let d = pa.decide(
            "secretsmanager.ap-northeast-1.amazonaws.com",
            443,
            &addrs(&["10.0.40.12"], 443),
            None,
        );
        assert_eq!(d, Decision::Allowed("injection rule".to_owned()));
    }

    #[test]
    fn legacy_flag_allows_any_private_ip_with_a_warning() {
        let pa = allow(true, &[]);
        assert!(pa.legacy_warning().is_some());
        assert!(control_plane().legacy_warning().is_none());
        assert!(pa.exempts("anything.internal", 443));
        assert!(matches!(
            pa.decide("anything.internal", 443, &addrs(&["10.0.0.9"], 443), Some(ip("10.0.0.1"))),
            Decision::Allowed(_)
        ));
        let d = pa.decide("imds", 80, &addrs(&["169.254.169.254"], 80), None);
        assert!(matches!(d, Decision::Denied(_)), "link-local stays denied: {d:?}");
    }

    #[test]
    fn link_local_denied_even_when_listed() {
        let pa = allow(false, &[("GUARD_PROXY_PRIVATE_ALLOW", "169.254.169.254,metadata.lan")]);
        let d = pa.decide("metadata.lan", 80, &addrs(&["10.0.0.2", "169.254.169.254"], 80), None);
        assert!(matches!(d, Decision::Denied(_)), "{d:?}");
    }

    #[test]
    fn public_hosts_unchanged() {
        let pa = control_plane();
        let gh = addrs(&["140.82.121.4"], 443);
        assert_eq!(pa.decide("api.github.com", 443, &gh, None), Decision::Public);
        let other_public_dst = Some(ip("140.82.121.5"));
        assert_eq!(pa.decide("api.github.com", 443, &gh, other_public_dst), Decision::Public);
        assert!(!pa.exempts("api.github.com", 443));
    }

    #[test]
    fn url_entries_carry_the_scheme_port() {
        let e = url_entry("https://user:pw@Cctui.Dorsk.Dev/v1#x", "K").unwrap();
        assert_eq!((e.host.as_str(), e.port), ("cctui.dorsk.dev", Some(443)));
        let e = url_entry("http://cctui:8080", "K").unwrap();
        assert_eq!((e.host.as_str(), e.port), ("cctui", Some(8080)));
        let e = url_entry("http://[fd00::1]/", "K").unwrap();
        assert_eq!((e.host.as_str(), e.port), ("fd00::1", Some(80)));
        let e = url_entry("https://cctui.dorsk.dev?x=@evil.com", "K").unwrap();
        assert_eq!((e.host.as_str(), e.port), ("cctui.dorsk.dev", Some(443)));
        let e = url_entry("https://a@b@host.example/x", "K").unwrap();
        assert_eq!(e.host, "host.example");
        assert!(url_entry("not a url", "K").is_none());
        assert!(url_entry("", "K").is_none());
    }

    #[test]
    fn seeded_file_is_read_and_reloaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private-allow");
        let seeded = "# seeded\ncctui.dev.svc:8080 CCTUI_BASE_URL\n\nhooks.lan:*\n";
        std::fs::write(&path, seeded).unwrap();
        let pa = PrivateAllow::build(false, |_| None, &[], Some(path.clone()));
        assert_eq!(
            pa.decide("cctui.dev.svc", 8080, &addrs(&[CLUSTER_IP], 8080), None),
            Decision::Allowed("control-plane list: CCTUI_BASE_URL".to_owned())
        );
        assert!(pa.exempts("hooks.lan", 9000));

        std::fs::write(&path, "other.lan:443\n").unwrap();
        let later = SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options().write(true).open(&path).unwrap().set_modified(later).unwrap();
        assert!(!pa.exempts("cctui.dev.svc", 8080));
        assert!(pa.exempts("other.lan", 443));

        std::fs::remove_file(&path).unwrap();
        assert!(!pa.exempts("other.lan", 443));
    }

    #[test]
    fn missing_file_is_an_empty_list() {
        let dir = tempfile::tempdir().unwrap();
        let pa = PrivateAllow::build(false, |_| None, &[], Some(dir.path().join("absent")));
        assert!(!pa.exempts("cctui.dev.svc", 8080));
    }

    #[tokio::test]
    async fn resolve_binds_literals_offline() {
        let pa = allow(false, &[("GUARD_PROXY_PRIVATE_ALLOW", "10.9.9.9:443,169.254.169.254:80")]);
        assert_eq!(pa.resolve("10.9.9.9", 443, None).await.unwrap(), addrs(&["10.9.9.9"], 443));
        assert!(pa.resolve("10.9.9.8", 443, None).await.is_err(), "unlisted private literal");
        assert!(pa.resolve("10.9.9.9", 443, Some(ip("10.9.9.8"))).await.is_err(), "other dst");
        assert!(pa.resolve("169.254.169.254", 80, None).await.is_err(), "metadata");
        assert_eq!(
            pa.resolve("140.82.121.4", 443, None).await.unwrap(),
            addrs(&["140.82.121.4"], 443)
        );
    }

    #[test]
    fn dual_stack_dials_ipv4_before_ipv6() {
        let mut got = addrs(
            &["2606:4700::6810:522", "2606:4700::6810:a22", "104.16.0.34", "104.16.10.34"],
            443,
        );
        dial_order(&mut got, None);
        let want = addrs(
            &["104.16.0.34", "104.16.10.34", "2606:4700::6810:522", "2606:4700::6810:a22"],
            443,
        );
        assert_eq!(got, want);
    }

    #[test]
    fn original_destination_is_dialed_first() {
        let mut got = addrs(&["2606:4700::6810:522", "104.16.0.34", "104.16.10.34"], 443);
        dial_order(&mut got, Some(ip("104.16.10.34")));
        assert_eq!(got, addrs(&["104.16.10.34", "104.16.0.34", "2606:4700::6810:522"], 443));
    }

    #[tokio::test]
    async fn legacy_resolve_permits_private() {
        let pa = allow(true, &[]);
        assert_eq!(pa.resolve("127.0.0.1", 8080, None).await.unwrap(), addrs(&["127.0.0.1"], 8080));
        assert!(pa.resolve("169.254.169.254", 80, None).await.is_err());
    }
}
