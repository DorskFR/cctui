//! `cctui-dispatcher-docker` `dispatcher.toml`; loaded and saved through
//! [`DispatcherConfig`].

use cctui_dispatcher_core::DispatcherConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    /// The enrollment key minted by the server (`sha256` stored server-side);
    /// presented on `dispatcher/auth` + as the `dispatcher/ws` token.
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
}

impl DispatcherConfig for Config {
    const ENROLL_HINT: &'static str =
        "cctui-dispatcher-docker enroll --server-url <url> --token <token> --name <name> --image <image>";

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
