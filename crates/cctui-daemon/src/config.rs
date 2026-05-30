//! Daemon on-disk configuration.
//!
//! Lives at `$XDG_CONFIG_HOME/cctui/daemon.toml` (or
//! `~/.config/cctui/daemon.toml`). Written by `cctui-daemon enroll`; read
//! by `cctui-daemon run`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    pub machine_key: String,
    pub machine_id: Option<uuid::Uuid>,
}

impl Config {
    #[must_use]
    pub fn default_path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("cctui").join("daemon.toml")
    }

    pub fn load_from(path: &PathBuf) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "no config at {} — this machine is not enrolled yet. \
                     Run `cctui-daemon enroll --server-url <url> --token <token> --name <name>` first.",
                    path.display()
                )
            } else {
                anyhow::Error::new(err).context(format!("reading {}", path.display()))
            }
        })?;
        Ok(toml::from_str(&raw)?)
    }

    /// Whether a config file exists at `path`. Used by `status` to report
    /// enrolment state without surfacing a raw I/O error.
    #[must_use]
    pub fn exists_at(path: &Path) -> bool {
        path.exists()
    }

    pub fn save_to(&self, path: &PathBuf) -> anyhow::Result<()> {
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
