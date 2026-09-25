//! On-disk `dispatcher.toml`, shared by every platform crate.
//!
//! Lives at `$XDG_CONFIG_HOME/cctui/dispatcher.toml` (or
//! `~/.config/cctui/dispatcher.toml`). Written by `<bin> enroll`; read by
//! `<bin> run`. Each platform keeps only its struct and enroll hint; the file IO
//! lives here.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

pub trait DispatcherConfig: Serialize + DeserializeOwned {
    /// Enroll command line shown when no config exists yet.
    const ENROLL_HINT: &'static str;

    fn server_url(&self) -> &str;
    fn dispatcher_key(&self) -> &str;
    fn dispatcher_id(&self) -> Option<uuid::Uuid>;
    fn worker_cctui_url(&self) -> Option<&str>;

    /// The URL injected into spawned workers as `CCTUI_URL`, falling back to the
    /// dispatcher's own `server_url`.
    fn worker_url(&self) -> &str {
        self.worker_cctui_url().unwrap_or_else(|| self.server_url())
    }

    #[must_use]
    fn default_path() -> PathBuf {
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("cctui").join("dispatcher.toml")
    }

    #[must_use]
    fn exists_at(path: &Path) -> bool {
        path.exists()
    }

    fn load_from(path: &Path) -> anyhow::Result<Self> {
        let raw = std::fs::read_to_string(path).map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                anyhow::anyhow!(
                    "no config at {} — this dispatcher is not enrolled yet. Run `{}` first.",
                    path.display(),
                    Self::ENROLL_HINT
                )
            } else {
                anyhow::Error::new(err).context(format!("reading {}", path.display()))
            }
        })?;
        Ok(toml::from_str(&raw)?)
    }

    fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        write_private(path, toml::to_string_pretty(self)?.as_bytes())
    }
}

/// Atomically replace `path` with `contents`, readable by the owner only.
///
/// The bytes go to a sibling tempfile created with mode 0600, which is then
/// renamed over `path`, so the secret is never visible with a wider mode.
pub fn write_private(path: &Path, contents: &[u8]) -> anyhow::Result<()> {
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
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Serialize, Deserialize)]
    struct Probe {
        server_url: String,
        dispatcher_key: String,
        dispatcher_id: Option<uuid::Uuid>,
        #[serde(default)]
        worker_cctui_url: Option<String>,
    }

    impl DispatcherConfig for Probe {
        const ENROLL_HINT: &'static str = "probe enroll --server-url <url>";
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

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!("cctui-dispatcher-cfg-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn save_then_load_round_trips_with_owner_only_mode() {
        let dir = scratch();
        let path = dir.join("nested").join("dispatcher.toml");
        let cfg = Probe {
            server_url: "https://s.example.test".to_owned(),
            dispatcher_key: "secret-key".to_owned(),
            dispatcher_id: Some(uuid::Uuid::nil()),
            worker_cctui_url: None,
        };
        assert!(!Probe::exists_at(&path));
        cfg.save_to(&path).unwrap();
        assert!(Probe::exists_at(&path));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let loaded = Probe::load_from(&path).unwrap();
        assert_eq!(loaded.dispatcher_key(), "secret-key");
        assert_eq!(loaded.dispatcher_id(), Some(uuid::Uuid::nil()));
        assert_eq!(loaded.worker_url(), "https://s.example.test");
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != "dispatcher.toml")
            .collect();
        assert!(leftovers.is_empty(), "tempfile left behind: {leftovers:?}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn overwriting_a_world_readable_file_leaves_it_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = scratch();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("dispatcher.toml");
        std::fs::write(&path, "old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&path, b"new").unwrap();
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_config_error_carries_the_enroll_hint() {
        let err = Probe::load_from(&scratch().join("dispatcher.toml")).unwrap_err().to_string();
        assert!(err.contains("not enrolled yet"), "{err}");
        assert!(err.contains(Probe::ENROLL_HINT), "{err}");
    }

    #[test]
    fn worker_url_prefers_the_override() {
        let cfg = Probe {
            server_url: "https://s.example.test".to_owned(),
            dispatcher_key: "k".to_owned(),
            dispatcher_id: None,
            worker_cctui_url: Some("http://in-cluster:8700".to_owned()),
        };
        assert_eq!(cfg.worker_url(), "http://in-cluster:8700");
    }
}
