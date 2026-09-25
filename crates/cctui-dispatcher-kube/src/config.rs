//! `cctui-dispatcher-kube` `dispatcher.toml`; loaded and saved through
//! [`DispatcherConfig`].

use cctui_dispatcher_core::DispatcherConfig;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    /// The enrollment key minted by the server (`sha256` stored server-side);
    /// presented on `dispatcher/auth` + as the `dispatcher/ws` Bearer credential.
    pub dispatcher_key: String,
    pub dispatcher_id: Option<uuid::Uuid>,
    /// Namespace the worker Job + its `WorkerProfile` resources live in.
    pub namespace: String,
    /// `WorkerProfile` instantiated when a dispatch selects none by name.
    /// Defaulted so pre-profile configs (which carry an ignored `source_cronjob`
    /// instead) still parse; re-enroll to populate it.
    #[serde(default)]
    pub default_profile: String,
    /// `CCTUI_URL` injected into spawned workers so their daemon dials back.
    /// Defaults to `server_url` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_cctui_url: Option<String>,
}

impl DispatcherConfig for Config {
    const ENROLL_HINT: &'static str = "cctui-dispatcher-kube enroll --server-url <url> --token <token> --name <name> --namespace <ns> --default-profile <name>";

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
