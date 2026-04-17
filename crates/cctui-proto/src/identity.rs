//! Identity config loaded from `$XDG_CONFIG_HOME/cctui/` (defaults to
//! `~/.config/cctui/`).
//!
//! Two files are supported:
//!   - `machine.json` — minted by `cctui-admin enroll` on each agent host;
//!     consumed by the channel to authenticate as a `Machine`.
//!   - `user.json` — minted by `cctui-admin user create` for human users;
//!     consumed by the TUI and by `cctui-admin enroll`.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MachineIdentity {
    pub server_url: String,
    pub machine_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hostname: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserIdentity {
    pub server_url: String,
    pub user_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// Resolve `$XDG_CONFIG_HOME/cctui/` (falls back to `$HOME/.config/cctui/`).
/// Returns `None` only if neither env var is set.
#[must_use]
pub fn config_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|p| p.join("cctui"))
}

#[must_use]
pub fn machine_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("machine.json"))
}

#[must_use]
pub fn user_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("user.json"))
}

#[must_use]
pub fn load_machine() -> Option<MachineIdentity> {
    let path = machine_path()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[must_use]
pub fn load_user() -> Option<UserIdentity> {
    let path = user_path()?;
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Write identity file with `0600` perms (best-effort on unix).
pub fn save_machine(id: &MachineIdentity) -> std::io::Result<PathBuf> {
    let path = machine_path()
        .ok_or_else(|| std::io::Error::other("could not resolve config dir (HOME unset?)"))?;
    write_secure(&path, &serde_json::to_vec_pretty(id)?)?;
    Ok(path)
}

pub fn save_user(id: &UserIdentity) -> std::io::Result<PathBuf> {
    let path = user_path()
        .ok_or_else(|| std::io::Error::other("could not resolve config dir (HOME unset?)"))?;
    write_secure(&path, &serde_json::to_vec_pretty(id)?)?;
    Ok(path)
}

fn write_secure(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_machine() {
        let id = MachineIdentity {
            server_url: "https://example".into(),
            machine_key: "cctui_m_xxx".into(),
            machine_id: Some("uuid".into()),
            hostname: Some("host".into()),
        };
        let s = serde_json::to_string(&id).unwrap();
        let back: MachineIdentity = serde_json::from_str(&s).unwrap();
        assert_eq!(back.machine_key, "cctui_m_xxx");
    }

    #[test]
    fn machine_path_under_cctui_dir() {
        if let Some(p) = machine_path() {
            assert!(p.ends_with("cctui/machine.json"));
        }
    }
}
