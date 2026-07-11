//! Dispatcher on-disk configuration.
//!
//! Lives at `$XDG_CONFIG_HOME/cctui/dispatcher.toml` (or
//! `~/.config/cctui/dispatcher.toml`). Written by
//! `cctui-dispatcher-apple enroll`; read by `cctui-dispatcher-apple run`.
//! Mirror of the daemon's `daemon.toml` (CCT-248 enrollment spec) — an enrolled
//! dispatcher is a peer of a machine, so its identity persists the same way.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const DEFAULT_CONTAINER_BIN: &str = "container";
const DEFAULT_SECRET_MOUNT_PATH: &str = "/run/cctui/machine_key";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server_url: String,
    /// The enrollment key minted by the server (`sha256` stored server-side);
    /// presented on `dispatcher/auth` + as the `dispatcher/ws` token.
    pub dispatcher_key: String,
    pub dispatcher_id: Option<uuid::Uuid>,
    /// Worker OCI image to boot on dispatch.
    pub image: String,
    /// `CCTUI_URL` injected into the worker so its daemon dials back. Defaults
    /// to `server_url` when unset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_cctui_url: Option<String>,
    /// Optional container network to attach spawned micro-VMs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
    /// Path to the `container` binary. Overridable so tests / non-default
    /// installs work; defaults to `container` on `PATH`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container_bin: Option<String>,
    /// Optional extra bind mounts (`host:guest[:ro]`) for spawned containers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mounts: Vec<String>,
    /// Optional repo mount (`host:guest`). When set the guest sees the repo at
    /// `guest` and is told to `git pull --depth 1` there at boot (CCT-280).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_mount: Option<String>,
    /// Guest path the machine-key file is mounted at (`CCTUI_MACHINE_KEY_FILE`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_mount_path: Option<String>,
    /// Host directory the per-session secret file is written to before it is
    /// mounted. Defaults to the system temp dir.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_dir: Option<PathBuf>,
    /// Deliver the machine key as a plain env var instead of a mounted file. The
    /// file is preferred (a token is visible in `container inspect` + the guest
    /// process list, CCT-245); this exists only for hosts where a file mount is
    /// impractical.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret_via_env: bool,
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
                     Run `cctui-dispatcher-apple enroll --server-url <url> --token <token> \
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

    #[must_use]
    pub fn container_bin(&self) -> &str {
        self.container_bin.as_deref().unwrap_or(DEFAULT_CONTAINER_BIN)
    }

    #[must_use]
    pub fn secret_mount_path(&self) -> &str {
        self.secret_mount_path.as_deref().unwrap_or(DEFAULT_SECRET_MOUNT_PATH)
    }

    #[must_use]
    pub fn secret_dir(&self) -> PathBuf {
        self.secret_dir.clone().unwrap_or_else(std::env::temp_dir)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_minimal_config_with_defaults() {
        let toml_src = r#"
            server_url = "https://cctui.example.test"
            dispatcher_key = "key-abc"
            image = "registry.example.test/cctui-worker:latest"
        "#;
        let cfg: Config = toml::from_str(toml_src).unwrap();
        assert_eq!(cfg.server_url, "https://cctui.example.test");
        assert_eq!(cfg.dispatcher_key, "key-abc");
        assert_eq!(cfg.image, "registry.example.test/cctui-worker:latest");
        // Defaults kick in for the optional surface.
        assert_eq!(cfg.worker_url(), "https://cctui.example.test");
        assert_eq!(cfg.container_bin(), "container");
        assert_eq!(cfg.secret_mount_path(), "/run/cctui/machine_key");
        assert!(!cfg.secret_via_env);
        assert!(cfg.mounts.is_empty());
        assert!(cfg.repo_mount.is_none());
    }

    #[test]
    fn parses_full_config_and_overrides() {
        let toml_src = r#"
            server_url = "https://s.example.test"
            dispatcher_key = "k"
            image = "img:1"
            worker_cctui_url = "https://worker.example.test"
            network = "cctui-net"
            container_bin = "/opt/apple/bin/container"
            mounts = ["/host/cache:/cache:ro"]
            repo_mount = "/host/repo:/workspace/repo"
            secret_mount_path = "/secrets/key"
            secret_via_env = true
        "#;
        let cfg: Config = toml::from_str(toml_src).unwrap();
        assert_eq!(cfg.worker_url(), "https://worker.example.test");
        assert_eq!(cfg.network.as_deref(), Some("cctui-net"));
        assert_eq!(cfg.container_bin(), "/opt/apple/bin/container");
        assert_eq!(cfg.mounts, vec!["/host/cache:/cache:ro".to_owned()]);
        assert_eq!(cfg.repo_mount.as_deref(), Some("/host/repo:/workspace/repo"));
        assert_eq!(cfg.secret_mount_path(), "/secrets/key");
        assert!(cfg.secret_via_env);
    }

    #[test]
    fn save_then_load_is_stable() {
        let dir = std::env::temp_dir().join(format!("cctui-apple-cfg-{}", uuid::Uuid::new_v4()));
        let path = dir.join("dispatcher.toml");
        let cfg = Config {
            server_url: "https://s.example.test".to_owned(),
            dispatcher_key: "secret-key".to_owned(),
            dispatcher_id: Some(uuid::Uuid::nil()),
            image: "img:2".to_owned(),
            worker_cctui_url: None,
            network: None,
            container_bin: None,
            mounts: vec![],
            repo_mount: None,
            secret_mount_path: None,
            secret_dir: None,
            secret_via_env: false,
        };
        cfg.save_to(&path).unwrap();
        let loaded = Config::load_from(&path).unwrap();
        assert_eq!(loaded.dispatcher_key, "secret-key");
        assert_eq!(loaded.image, "img:2");
        std::fs::remove_dir_all(&dir).ok();
    }
}
