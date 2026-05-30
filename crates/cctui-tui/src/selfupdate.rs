//! Silent self-update on TUI startup.
//!
//! Flow (`maybe_update`):
//!   1. Compare `CARGO_PKG_VERSION` with `GET {server}/api/v1/version`.
//!   2. If server is newer, download `cctui-{os}-{arch}` from the matching
//!      GitHub release, atomic-rename over `current_exe()`.
//!   3. If `install::SETTINGS_SCHEMA_VERSION` exceeds the marker file,
//!      re-apply hook/MCP config.
//!   4. `exec()` into the freshly-written binary with `CCTUI_UPDATED=1` so we
//!      don't recurse on the next launch.
//!
//! Any failure before the rename is silent and non-fatal — the old binary
//! continues to run. We never block startup for more than a couple of seconds
//! of network work.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::install;

pub const UPDATED_ENV: &str = "CCTUI_UPDATED";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_REPO: &str = "DorskFR/cctui";
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(serde::Deserialize)]
struct ServerVersion {
    version: String,
}

#[must_use]
pub const fn asset_for(os: &str, arch: &str) -> Option<&'static str> {
    match (os.as_bytes(), arch.as_bytes()) {
        (b"linux", b"x86_64") => Some("cctui-linux-amd64"),
        (b"linux", b"aarch64") => Some("cctui-linux-arm64"),
        (b"macos", b"aarch64") => Some("cctui-darwin-arm64"),
        _ => None,
    }
}

/// Returns `true` when `server_ver` is a strictly higher semver than `local_ver`.
/// Any parse failure (dev builds, pre-release weirdness) yields `false` so we
/// stay conservative and never auto-update from an unparseable base.
#[must_use]
pub fn should_update(local_ver: &str, server_ver: &str) -> bool {
    match (semver::Version::parse(local_ver), semver::Version::parse(server_ver)) {
        (Ok(local), Ok(server)) => server > local,
        _ => false,
    }
}

fn repo() -> String {
    std::env::var("CCTUI_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

fn release_url(asset: &str, tag: Option<&str>) -> String {
    let repo = repo();
    tag.map_or_else(
        || format!("https://github.com/{repo}/releases/latest/download/{asset}"),
        |t| format!("https://github.com/{repo}/releases/download/{t}/{asset}"),
    )
}

/// Check if the exe path is writable by the current user by creating a sibling
/// temp file in the same directory.
fn exe_dir_writable(exe: &Path) -> bool {
    let Some(parent) = exe.parent() else { return false };
    let probe = parent.join(".cctui-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

async fn fetch_server_version(server_url: &str) -> Result<String> {
    let client = reqwest::Client::builder().timeout(VERSION_TIMEOUT).build()?;
    let url = format!("{}/api/v1/version", server_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await?.error_for_status()?;
    let info: ServerVersion = resp.json().await?;
    Ok(info.version)
}

async fn download_to(path: &Path, url: &str) -> Result<()> {
    let client = reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT).build()?;
    let resp = client.get(url).send().await?.error_for_status()?;
    let bytes = resp.bytes().await?;
    std::fs::write(path, &bytes).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

async fn swap_binary(target_tag: Option<&str>) -> Result<PathBuf> {
    let asset = asset_for(std::env::consts::OS, std::env::consts::ARCH)
        .ok_or_else(|| anyhow!("unsupported os/arch"))?;
    let current = std::env::current_exe().context("resolve current exe")?;
    if !exe_dir_writable(&current) {
        bail!("current exe directory is not writable: {}", current.display());
    }
    let staging = current.with_extension("new");
    download_to(&staging, &release_url(asset, target_tag)).await?;
    std::fs::rename(&staging, &current)
        .with_context(|| format!("rename {} -> {}", staging.display(), current.display()))?;
    Ok(current)
}

fn maybe_reapply_settings(server_url: &str, fallback_token: &str, bin_path: &Path) {
    if install::SETTINGS_SCHEMA_VERSION <= install::read_schema_marker() {
        return;
    }
    if let Err(e) = install::apply_settings(server_url, fallback_token, bin_path) {
        eprintln!("[cctui] settings re-apply failed: {e}");
        return;
    }
    if let Err(e) = install::write_schema_marker(install::SETTINGS_SCHEMA_VERSION) {
        eprintln!("[cctui] writing schema marker failed: {e}");
    }
}

/// Replace the current process with the binary at `exe`, forwarding CLI args
/// and setting `CCTUI_UPDATED=1` so the new process skips the update check.
#[cfg(unix)]
fn exec_new(exe: &Path) -> ! {
    use std::os::unix::process::CommandExt;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args_os().skip(1));
    cmd.env(UPDATED_ENV, "1");
    // If exec returns, it failed. Fall back to exiting with the error.
    let err = cmd.exec();
    eprintln!("[cctui] exec failed: {err}");
    std::process::exit(1);
}

#[cfg(not(unix))]
fn exec_new(_: &Path) -> ! {
    std::process::exit(0);
}

async fn update_inner(server_url: &str, target_tag: Option<&str>) -> Result<()> {
    let new_exe = swap_binary(target_tag).await?;
    let fallback_token =
        cctui_proto::identity::load_machine().map(|m| m.machine_key).unwrap_or_default();
    maybe_reapply_settings(server_url, &fallback_token, &new_exe);
    exec_new(&new_exe);
}

fn tag_override() -> Option<String> {
    std::env::var("CCTUI_TAG").ok().filter(|t| !t.is_empty() && t != "latest")
}

fn clear_updated_flag() {
    #[allow(unsafe_code)]
    unsafe {
        std::env::remove_var(UPDATED_ENV);
    }
}

/// Called once at TUI startup. Silent + best-effort — any error leaves the
/// user running the current binary.
pub async fn maybe_update(server_url: &str) {
    if std::env::var(UPDATED_ENV).is_ok() {
        clear_updated_flag();
        return;
    }
    let Ok(server_version) = fetch_server_version(server_url).await else { return };
    if !should_update(CURRENT_VERSION, &server_version) {
        // Still run the schema-only reapply if needed — covers users who
        // manually updated the binary but never re-ran install.sh.
        if install::SETTINGS_SCHEMA_VERSION > install::read_schema_marker()
            && let Ok(exe) = std::env::current_exe()
        {
            let fallback_token =
                cctui_proto::identity::load_machine().map(|m| m.machine_key).unwrap_or_default();
            maybe_reapply_settings(server_url, &fallback_token, &exe);
        }
        return;
    }
    eprintln!("[cctui] updating {CURRENT_VERSION} -> {server_version}…");
    let tag = tag_override().or_else(|| Some(format!("v{server_version}")));
    if let Err(e) = update_inner(server_url, tag.as_deref()).await {
        eprintln!("[cctui] update failed: {e}");
    }
}

/// Invoked by the `cctui update` subcommand. Always re-downloads from the
/// latest release (or `$CCTUI_TAG`) and re-applies settings unconditionally.
pub async fn force_update(server_url: &str) -> Result<()> {
    clear_updated_flag();
    eprintln!("[cctui] forcing update from {}", repo());
    let new_exe = swap_binary(tag_override().as_deref()).await?;
    let fallback_token =
        cctui_proto::identity::load_machine().map(|m| m.machine_key).unwrap_or_default();
    if let Err(e) = install::apply_settings(server_url, &fallback_token, &new_exe) {
        eprintln!("[cctui] settings re-apply failed: {e}");
    } else {
        let _ = install::write_schema_marker(install::SETTINGS_SCHEMA_VERSION);
    }
    eprintln!("[cctui] update complete -> {}", new_exe.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names() {
        assert_eq!(asset_for("linux", "x86_64"), Some("cctui-linux-amd64"));
        assert_eq!(asset_for("linux", "aarch64"), Some("cctui-linux-arm64"));
        assert_eq!(asset_for("macos", "aarch64"), Some("cctui-darwin-arm64"));
        assert_eq!(asset_for("macos", "x86_64"), None);
        assert_eq!(asset_for("windows", "x86_64"), None);
    }

    #[test]
    fn version_comparison() {
        assert!(should_update("0.1.5", "0.1.6"));
        assert!(should_update("0.1.5", "0.2.0"));
        assert!(!should_update("0.1.6", "0.1.6"));
        assert!(!should_update("0.1.7", "0.1.6"));
        assert!(!should_update("not-semver", "0.1.6"));
        assert!(!should_update("0.1.5", "bad"));
    }

    #[test]
    fn release_url_formats() {
        // Test relies on default CCTUI_REPO — don't override here.
        let base = format!("https://github.com/{}/releases", repo());
        assert_eq!(
            release_url("cctui-linux-amd64", None),
            format!("{base}/latest/download/cctui-linux-amd64")
        );
        assert_eq!(
            release_url("cctui-linux-amd64", Some("v0.1.6")),
            format!("{base}/download/v0.1.6/cctui-linux-amd64")
        );
    }
}
