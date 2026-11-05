//! The lateral-movement target list, generated from the live service inventory.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub host: String,
    pub port: u16,
}

impl Target {
    /// `host:port`, bracketing an IPv6 host.
    #[must_use]
    pub fn authority(&self) -> String {
        if self.host.contains(':') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }

    fn is_sanctioned(&self, sanctioned: &[String]) -> bool {
        let authority = self.authority();
        sanctioned.iter().any(|s| {
            s == &self.id
                || s == &authority
                || s.strip_suffix("/*").is_some_and(|ns| {
                    self.id.strip_prefix(ns).is_some_and(|rest| rest.starts_with('/'))
                })
        })
    }
}

fn tcp_port(port: &Value) -> Option<u16> {
    let number = match port {
        Value::Number(n) => n.as_u64()?,
        Value::Object(o) => {
            let tcp = o
                .get("protocol")
                .and_then(Value::as_str)
                .is_none_or(|p| p.eq_ignore_ascii_case("tcp"));
            if !tcp {
                return None;
            }
            o.get("port")?.as_u64()?
        }
        _ => return None,
    };
    u16::try_from(number).ok().filter(|n| *n != 0)
}

fn tcp_ports(ports: Option<&Value>) -> Vec<u16> {
    ports.and_then(Value::as_array).map_or_else(Vec::new, |a| a.iter().filter_map(tcp_port).collect())
}

fn kube_ips(item: &Value) -> Vec<String> {
    let listed: Vec<String> = item
        .pointer("/spec/clusterIPs")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect());
    let ips = if listed.is_empty() {
        item.pointer("/spec/clusterIP").and_then(Value::as_str).map(str::to_owned).into_iter().collect()
    } else {
        listed
    };
    ips.into_iter().filter(|ip| !ip.is_empty() && ip != "None").collect()
}

fn kube_targets(items: &[Value]) -> Vec<Target> {
    let mut out = Vec::new();
    for item in items {
        if item.pointer("/spec/type").and_then(Value::as_str) == Some("ExternalName") {
            continue;
        }
        let ns = item.pointer("/metadata/namespace").and_then(Value::as_str).unwrap_or("default");
        let name = item.pointer("/metadata/name").and_then(Value::as_str).unwrap_or_default();
        let ports = tcp_ports(item.pointer("/spec/ports"));
        for host in kube_ips(item) {
            for port in &ports {
                out.push(Target { id: format!("{ns}/{name}"), host: host.clone(), port: *port });
            }
        }
    }
    out
}

fn generic_targets(services: &[Value]) -> Vec<Target> {
    let mut out = Vec::new();
    for svc in services {
        let name = svc.get("name").and_then(Value::as_str).unwrap_or_default();
        let Some(host) = svc.get("host").and_then(Value::as_str).filter(|h| !h.is_empty()) else {
            continue;
        };
        for port in tcp_ports(svc.get("ports")) {
            out.push(Target { id: name.to_owned(), host: host.to_owned(), port });
        }
    }
    out
}

/// Every unsanctioned TCP destination in `inventory`, sorted and deduplicated.
///
/// `inventory` is a Kubernetes `ServiceList` (`kubectl get svc -A -o json`) or
/// `{"services":[{"name","host","ports":[...]}]}` for other dispatchers.
/// Headless and `ExternalName` services have no address to reach and are
/// skipped. A `sanctioned` entry is a target id (`ns/name`), `ns/*`, or an
/// authority (`host:port`).
#[must_use]
pub fn from_inventory(inventory: &Value, sanctioned: &[String]) -> Vec<Target> {
    let mut all = inventory.get("items").and_then(Value::as_array).map_or_else(
        || {
            inventory
                .get("services")
                .and_then(Value::as_array)
                .map_or_else(Vec::new, |s| generic_targets(s.as_slice()))
        },
        |items| kube_targets(items.as_slice()),
    );
    all.retain(|t| !t.is_sanctioned(sanctioned));
    all.sort();
    all.dedup();
    all
}

/// One `authority id` line per target, at most `max`; also returns how many
/// were left out.
#[must_use]
pub fn render(targets: &[Target], max: usize) -> (String, usize) {
    use std::fmt::Write as _;
    let mut out = String::new();
    for t in targets.iter().take(max) {
        let _ = writeln!(out, "{} {}", t.authority(), t.id);
    }
    (out, targets.len().saturating_sub(max))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn sample_service_list() -> Value {
        json!({
            "kind": "ServiceList",
            "items": [
                {"metadata": {"name": "db", "namespace": "data"},
                 "spec": {"type": "ClusterIP", "clusterIP": "10.96.0.10",
                          "ports": [{"port": 5432, "protocol": "TCP"}, {"port": 53, "protocol": "UDP"}]}},
                {"metadata": {"name": "api", "namespace": "app"},
                 "spec": {"clusterIP": "10.96.0.20", "clusterIPs": ["10.96.0.20", "fd00::20"],
                          "ports": [{"port": 8080}]}},
                {"metadata": {"name": "headless", "namespace": "app"},
                 "spec": {"clusterIP": "None", "ports": [{"port": 9000}]}},
                {"metadata": {"name": "ext", "namespace": "app"},
                 "spec": {"type": "ExternalName", "externalName": "example.com", "ports": [{"port": 443}]}},
                {"metadata": {"name": "server", "namespace": "cctui"},
                 "spec": {"clusterIP": "10.96.0.30", "ports": [{"port": 443}]}},
                {"metadata": {"name": "proxy", "namespace": "mesh"},
                 "spec": {"clusterIP": "10.96.0.40", "ports": [{"port": 15001}]}}
            ]
        })
    }

    #[test]
    fn a_service_list_yields_every_unsanctioned_tcp_address() {
        let sanctioned = vec!["cctui/server".to_owned(), "mesh/*".to_owned()];
        let got = from_inventory(&sample_service_list(), &sanctioned);
        let lines: Vec<String> = got.iter().map(|t| format!("{} {}", t.authority(), t.id)).collect();
        assert_eq!(
            lines,
            vec![
                "10.96.0.20:8080 app/api".to_owned(),
                "[fd00::20]:8080 app/api".to_owned(),
                "10.96.0.10:5432 data/db".to_owned(),
            ]
        );
    }

    #[test]
    fn an_authority_can_be_sanctioned_and_a_namespace_glob_does_not_match_a_prefix() {
        let sanctioned = vec!["10.96.0.10:5432".to_owned(), "ap/*".to_owned()];
        let got = from_inventory(&sample_service_list(), &sanctioned);
        assert!(got.iter().all(|t| t.id != "data/db"));
        assert!(got.iter().any(|t| t.id == "app/api"));
    }

    #[test]
    fn the_generic_inventory_takes_plain_and_object_ports() {
        let inv = json!({"services": [
            {"name": "cache", "host": "192.0.2.5", "ports": [6379, {"port": 6380, "protocol": "udp"}]},
            {"name": "nohost", "ports": [80]},
            {"name": "web", "host": "192.0.2.6", "ports": [{"port": 80}, 0, 70000]}
        ]});
        let got = from_inventory(&inv, &[]);
        assert_eq!(
            got,
            vec![
                Target { id: "cache".to_owned(), host: "192.0.2.5".to_owned(), port: 6379 },
                Target { id: "web".to_owned(), host: "192.0.2.6".to_owned(), port: 80 },
            ]
        );
    }

    #[test]
    fn an_unknown_inventory_shape_yields_no_targets() {
        assert!(from_inventory(&json!({"other": []}), &[]).is_empty());
    }

    #[test]
    fn render_caps_the_list_and_counts_what_it_left_out() {
        let got = from_inventory(&sample_service_list(), &[]);
        let (text, left_out) = render(&got, 2);
        assert_eq!(text.lines().count(), 2);
        assert_eq!(left_out, got.len() - 2);
        let (all, none_left) = render(&got, 100);
        assert_eq!(all.lines().count(), got.len());
        assert_eq!(none_left, 0);
    }
}
