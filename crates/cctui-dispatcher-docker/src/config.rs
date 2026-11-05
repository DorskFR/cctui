//! `cctui-dispatcher-docker` `dispatcher.toml`; loaded and saved through
//! [`DispatcherConfig`].

use cctui_dispatcher_core::DispatcherConfig;
use serde::{Deserialize, Serialize};

/// How a cell's agent egress is confined.
///
/// `Transparent` grants the cell `NET_ADMIN` and pins `WORKER_NET_MODE=transparent`:
/// the root entrypoint installs the owner-based redirect into the in-cell
/// guard-proxy plus the filter-table rejects (link-local, non-TCP, IPv6), then
/// the supervisor drops every capability before the agent runs. A cell whose
/// iptables is unusable refuses to start. `Forward` only exports the proxy env,
/// which an agent can ignore; it is never chosen implicitly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Egress {
    #[default]
    Transparent,
    Forward,
}

impl Egress {
    #[must_use]
    pub const fn net_mode(self) -> &'static str {
        match self {
            Self::Transparent => "transparent",
            Self::Forward => "forward",
        }
    }

    #[allow(clippy::trivially_copy_pass_by_ref)]
    const fn is_transparent(&self) -> bool {
        matches!(self, Self::Transparent)
    }
}

/// Per-cell confinement the dispatcher imposes, independent of the payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Confinement {
    pub egress: Egress,
    /// `false` sets `CCTUI_LANDLOCK_STRICT=0` in the cell, for docker hosts
    /// whose kernel or seccomp profile does not expose Landlock.
    pub landlock_strict: bool,
}

impl Default for Confinement {
    fn default() -> Self {
        Self { egress: Egress::Transparent, landlock_strict: true }
    }
}

const fn default_true() -> bool {
    true
}

#[allow(clippy::trivially_copy_pass_by_ref)]
const fn is_true(v: &bool) -> bool {
    *v
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    /// The enrollment key minted by the server (`sha256` stored server-side);
    /// presented on `dispatcher/auth` + as the `dispatcher/ws` Bearer credential.
    pub dispatcher_key: String,
    pub dispatcher_id: Option<uuid::Uuid>,
    /// Worker image to spawn on dispatch.
    pub image: String,
    /// `CCTUI_URL` injected into the worker so its daemon dials back. Defaults
    /// to `server_url` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_cctui_url: Option<String>,
    /// Optional docker network to attach spawned containers to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Optional docker host/socket. The standard local socket is used when
    /// unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docker_host: Option<String>,
    /// Optional bind mounts (`/host:/container[:ro]`) for spawned containers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<String>,
    /// Cell egress confinement; `transparent` unless explicitly `forward`.
    #[serde(default, skip_serializing_if = "Egress::is_transparent")]
    pub egress: Egress,
    /// Landlock is mandatory in the cell unless set to `false`.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub landlock_strict: bool,
}

impl Config {
    #[must_use]
    pub const fn confinement(&self) -> Confinement {
        Confinement { egress: self.egress, landlock_strict: self.landlock_strict }
    }
}

impl DispatcherConfig for Config {
    const ENROLL_HINT: &'static str = "cctui-dispatcher-docker enroll --server-url <url> --token <token> --name <name> --image <image>";

    fn server_url(&self) -> &str {
        &self.server_url
    }

    fn dispatcher_key(&self) -> &str {
        &self.dispatcher_key
    }

    fn dispatcher_id(&self) -> Option<uuid::Uuid> {
        self.dispatcher_id
    }

    fn worker_cctui_url(&self) -> Option<&str> {
        self.worker_cctui_url.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn minimal() -> serde_json::Value {
        json!({ "server_url": "https://s.example.test", "dispatcher_key": "k", "image": "img:1" })
    }

    #[test]
    fn an_existing_config_defaults_to_transparent_and_strict() {
        let cfg: Config = serde_json::from_value(minimal()).unwrap();
        assert_eq!(cfg.confinement(), Confinement::default());
        assert_eq!(cfg.egress, Egress::Transparent);
        assert!(cfg.landlock_strict);
    }

    #[test]
    fn forward_and_best_effort_landlock_are_explicit_opt_ins() {
        let mut v = minimal();
        v["egress"] = json!("forward");
        v["landlock_strict"] = json!(false);
        let cfg: Config = serde_json::from_value(v).unwrap();
        assert_eq!(cfg.egress, Egress::Forward);
        assert!(!cfg.landlock_strict);
        let saved = serde_json::to_value(&cfg).unwrap();
        assert_eq!(saved["egress"], "forward");
        assert_eq!(saved["landlock_strict"], false);
    }

    #[test]
    fn defaults_are_not_written_back() {
        let cfg: Config = serde_json::from_value(minimal()).unwrap();
        let saved = serde_json::to_value(&cfg).unwrap();
        assert!(saved.get("egress").is_none(), "{saved}");
        assert!(saved.get("landlock_strict").is_none(), "{saved}");
    }

    #[test]
    fn an_unknown_egress_is_rejected() {
        let mut v = minimal();
        v["egress"] = json!("none");
        assert!(serde_json::from_value::<Config>(v).is_err());
    }
}
