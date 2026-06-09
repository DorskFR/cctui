//! Dispatcher on-disk configuration.
//!
//! Lives at `$XDG_CONFIG_HOME/cctui/dispatcher.toml` (or
//! `~/.config/cctui/dispatcher.toml`). Written by
//! `cctui-dispatcher-docker enroll`; read by `cctui-dispatcher-docker run`.
//! Mirror of the daemon's `daemon.toml` (CCT-248 enrollment spec) — an enrolled
//! dispatcher is a peer of a machine, so its identity persists the same way.

use std::path::{Path, PathBuf};

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

impl Config {
    #[must_use]
    pub fn default_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("cctui")
            .join("dispatcher.toml")
    }

    pub fn load_from(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "no config at {} — this dispatcher is not enrolled yet. \
                     Run `cctui-dispatcher-docker enroll --server-url <url> --token <token> \
                     --name <name> --image <image>` first.",
                    path.display()
                )
            } else {
                anyhow::Error::new(err).context(format!("reading {}", path.display()))
            }
        })?;
        Ok(toml::from_str(&raw)?)
    }

    #[must_use]
    pub fn exists_at(path: &Path) -> bool {
        path.exists()
    }

    /// The URL injected into spawned workers as `CCTUI_URL`, falling back to the
    /// dispatcher's own `server_url`.
    #[must_use]
    pub fn worker_url(&self) -> &str {
        self.worker_cctui_url.as_deref().unwrap_or(&self.server_url)
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(path, raw)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_mode(0o600);
            std::fs::set_permissions(path, perms)?;
        }
        Ok(())
    }
}
