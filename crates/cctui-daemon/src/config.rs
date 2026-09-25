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
    /// Extra roots the linked-file viewer may read from, for the cases the
    /// session's cwd and job dir cannot infer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_file_roots: Vec<String>,
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

    /// Build a config purely from environment variables, for dispatched
    /// worker pods that are handed a shared machine key and never run `enroll`.
    /// `CCTUI_MACHINE_KEY` + (`CCTUI_SERVER_URL` or `CCTUI_URL`) are
    /// required; `machine_id` is unknown here (the server returns it from
    /// `daemon_auth`). Returns `None` when the key isn't set so the caller can
    /// fall back to the on-disk config.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let machine_key = std::env::var("CCTUI_MACHINE_KEY").ok().filter(|s| !s.is_empty())?;
        let server_url = std::env::var("CCTUI_SERVER_URL")
            .or_else(|_| std::env::var("CCTUI_URL"))
            .ok()
            .filter(|s| !s.is_empty())?;
        Some(Self { server_url, machine_key, machine_id: None, read_file_roots: Vec::new() })
    }

    /// Resolve config for `run`: prefer the env-provided shared key (dispatch
    /// pods), otherwise the on-disk config written by `enroll`.
    pub fn load_or_env(path: &PathBuf) -> anyhow::Result<Self> {
        if let Some(cfg) = Self::from_env() {
            tracing::info!("using machine key from environment (CCTUI_MACHINE_KEY)");
            return Ok(cfg);
        }
        Self::load_from(path)
    }

    /// Whether a config file exists at `path`. Used by `status` to report
    /// enrolment state without surfacing a raw I/O error.
    #[must_use]
    pub fn exists_at(path: &Path) -> bool {
        path.exists()
    }

    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        write_private(path, toml::to_string_pretty(self)?.as_bytes())
    }
}

/// Atomically replace `path` with `contents` through a sibling tempfile
/// created with mode 0600, so the key is never readable by anyone else.
fn write_private(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;

    let dir = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    std::fs::create_dir_all(dir)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple()));
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let result = (|| -> anyhow::Result<()> {
        let mut file = opts.open(&tmp)?;
        file.write_all(contents)?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(|e| e.context(format!("writing {}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_creates_an_owner_only_file_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cctui").join("daemon.toml");
        let cfg = Config {
            server_url: "https://s.example.test".to_owned(),
            machine_key: "secret-key".to_owned(),
            machine_id: None,
            read_file_roots: Vec::new(),
        };
        cfg.save_to(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        assert_eq!(Config::load_from(&path).unwrap().machine_key, "secret-key");
        assert_eq!(std::fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn save_over_a_world_readable_file_leaves_it_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("daemon.toml");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
    }
}
