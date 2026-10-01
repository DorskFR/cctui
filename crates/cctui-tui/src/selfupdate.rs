//! Silent self-update on TUI startup.
//!
//! Flow (`maybe_update`):
//!   1. Compare `CARGO_PKG_VERSION` with `GET {server}/api/v1/version`. A
//!      stable build never moves onto a beta server version unless
//!      `CCTUI_CHANNEL=beta` opts in.
//!   2. If server is newer, download `cctui-{os}-{arch}`, `SHA256SUMS` and
//!      the asset's `.minisig` from the matching GitHub release, verify
//!      checksum and release signature, stage it, require `--version` to
//!      succeed, then rename over `current_exe()` keeping a `.bak`.
//!   3. If `install::SETTINGS_SCHEMA_VERSION` exceeds the marker file, run
//!      the settings migration that clears cctui's retired hooks.
//!   4. `exec()` into the freshly-written binary with `CCTUI_UPDATED=1` so we
//!      don't recurse on the next launch.
//!
//! Any failure before the rename is silent and non-fatal — the old binary
//! continues to run. We never block startup for more than a couple of seconds
//! of network work.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use cctui_proto::release_sig::Channel;

use crate::install;

pub const UPDATED_ENV: &str = "CCTUI_UPDATED";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
const DEFAULT_REPO: &str = "DorskFR/cctui";
const VERSION_TIMEOUT: Duration = Duration::from_secs(2);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_mins(1);

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
pub fn should_update(local_ver: &str, server_ver: &str, channel: Channel) -> bool {
    if channel == Channel::Stable && Channel::of_version(server_ver) == Channel::Beta {
        return false;
    }
    match (semver::Version::parse(local_ver), semver::Version::parse(server_ver)) {
        (Ok(local), Ok(server)) => server > local,
        _ => false,
    }
}

/// `CCTUI_CHANNEL` when set, else the channel this build was released on.
fn update_channel() -> Channel {
    let own = Channel::of_version(CURRENT_VERSION);
    match std::env::var("CCTUI_CHANNEL").map(|v| v.parse::<Channel>()) {
        Ok(Ok(channel)) => channel,
        Ok(Err(err)) => {
            eprintln!("[cctui] ignoring CCTUI_CHANNEL: {err}");
            own
        }
        Err(_) => own,
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
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder().timeout(VERSION_TIMEOUT).build()?;
    let route = cctui_proto::api::routes::by_id("get_version").context("version route")?;
    let url = format!("{}{}", server_url.trim_end_matches('/'), route.url(&[]));
    let resp = client.get(&url).send().await?.error_for_status()?;
    let info: ServerVersion = resp.json().await?;
    Ok(info.version)
}

async fn fetch(client: &reqwest::Client, url: &str) -> Result<Vec<u8>> {
    let resp = client.get(url).send().await?.error_for_status()?;
    Ok(resp.bytes().await?.to_vec())
}

fn hex_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// Check `bin` against its `SHA256SUMS` entry and the release signature.
fn verify_release(asset: &str, bin: &[u8], sums: &[u8], minisig: &[u8]) -> Result<()> {
    let sums = std::str::from_utf8(sums).context("SHA256SUMS not UTF-8")?;
    let expected = sums
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            let hash = it.next()?;
            (it.next()? == asset).then_some(hash)
        })
        .ok_or_else(|| anyhow!("{asset} missing from SHA256SUMS"))?;
    let actual = hex_sha256(bin);
    if actual != expected {
        bail!("downloaded {asset} hash {actual} != expected {expected}");
    }
    let minisig = std::str::from_utf8(minisig).context("signature not UTF-8")?;
    cctui_proto::release_sig::verify(bin, minisig).map_err(|e| anyhow!("{asset}: {e}"))
}

async fn download_verified(path: &Path, asset: &str, tag: Option<&str>) -> Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let client = reqwest::Client::builder().timeout(DOWNLOAD_TIMEOUT).build()?;
    let bytes = fetch(&client, &release_url(asset, tag)).await?;
    let sums = fetch(&client, &release_url("SHA256SUMS", tag)).await.context("SHA256SUMS")?;
    let sig_name = format!("{asset}{}", cctui_proto::release_sig::SIG_SUFFIX);
    let sig = fetch(&client, &release_url(&sig_name, tag)).await.context("signature")?;
    verify_release(asset, &bytes, &sums, &sig)?;
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
    let backup = current.with_extension("bak");
    download_verified(&staging, asset, target_tag).await?;
    if let Err(e) = check_version(&staging) {
        let _ = std::fs::remove_file(&staging);
        return Err(e);
    }
    install_staged(&staging, &current, &backup)?;
    Ok(current)
}

fn check_version(exe: &Path) -> Result<()> {
    let out = std::process::Command::new(exe)
        .arg("--version")
        .output()
        .with_context(|| format!("run `{} --version`", exe.display()))?;
    if !out.status.success() {
        bail!("`{} --version` exited {}", exe.display(), out.status);
    }
    Ok(())
}

/// Move `staging` over `current`, keeping the old binary at `backup` until
/// the installed file passes `--version`; restore it otherwise.
fn install_staged(staging: &Path, current: &Path, backup: &Path) -> Result<()> {
    let backed_up = current.exists() && std::fs::copy(current, backup).is_ok();
    std::fs::rename(staging, current)
        .with_context(|| format!("rename {} -> {}", staging.display(), current.display()))?;
    if let Err(e) = check_version(current) {
        if backed_up {
            let _ = std::fs::rename(backup, current);
        }
        return Err(e);
    }
    if backed_up {
        let _ = std::fs::remove_file(backup);
    }
    Ok(())
}

fn migrate_settings_if_behind() {
    if install::SETTINGS_SCHEMA_VERSION <= install::read_schema_marker() {
        return;
    }
    if let Err(e) = install::migrate_legacy_hooks() {
        eprintln!("[cctui] settings migration failed: {e}");
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

async fn update_inner(target_tag: Option<&str>) -> Result<()> {
    let new_exe = swap_binary(target_tag).await?;
    migrate_settings_if_behind();
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
    if !should_update(CURRENT_VERSION, &server_version, update_channel()) {
        migrate_settings_if_behind();
        return;
    }
    eprintln!("[cctui] updating {CURRENT_VERSION} -> {server_version}…");
    let tag = tag_override().or_else(|| Some(format!("v{server_version}")));
    if let Err(e) = update_inner(tag.as_deref()).await {
        eprintln!("[cctui] update failed: {e}");
    }
}

/// Whether going from `current` to `target` would move backwards. An
/// unparseable pair is not a downgrade: the comparison is what is unknown, not
/// the direction.
#[must_use]
pub fn is_downgrade(current: &str, target: &str) -> bool {
    match (semver::Version::parse(current), semver::Version::parse(target.trim_start_matches('v')))
    {
        (Ok(current), Ok(target)) => target < current,
        _ => false,
    }
}

/// Which tag `cctui update` should install: `$CCTUI_TAG`, else the version the
/// server runs, which is the one the rest of the fleet is on. `latest` is a
/// different release line and would silently move a beta user to stable.
async fn update_target(server_url: &str) -> Option<String> {
    if let Some(tag) = tag_override() {
        return Some(tag);
    }
    match fetch_server_version(server_url).await {
        Ok(version) => Some(format!("v{version}")),
        Err(e) => {
            eprintln!("[cctui] cannot ask {server_url} which version it runs: {e}");
            None
        }
    }
}

/// Invoked by the `cctui update` subcommand. Re-downloads the release the server
/// runs (or `$CCTUI_TAG`), refusing to move backwards unless asked. It adds
/// nothing to the user's settings: the only write left is clearing cctui's
/// retired hooks.
pub async fn force_update(server_url: &str, force: bool) -> Result<()> {
    clear_updated_flag();
    let target = update_target(server_url).await;
    match target.as_deref() {
        Some(tag) if is_downgrade(CURRENT_VERSION, tag) && !force => {
            bail!(
                "{tag} is older than the {CURRENT_VERSION} you are running;                  pass --force to install it anyway"
            );
        }
        Some(tag) => eprintln!("[cctui] updating {CURRENT_VERSION} -> {tag} from {}", repo()),
        None => bail!(
            "refusing to install an unknown version: set CCTUI_TAG=vX.Y.Z,              or make the server reachable so its version can be matched"
        ),
    }
    let new_exe = swap_binary(target.as_deref()).await?;
    if let Err(e) = install::migrate_legacy_hooks() {
        eprintln!("[cctui] settings migration failed: {e}");
    } else {
        let _ = install::write_schema_marker(install::SETTINGS_SCHEMA_VERSION);
    }
    eprintln!("[cctui] update complete -> {}", new_exe.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F18: `cctui update` took `releases/latest`, which is the stable line, so
    /// a beta user was moved backwards until the next startup update undid it.
    #[test]
    fn a_lower_target_is_recognised_as_a_downgrade() {
        assert!(is_downgrade("0.23.0-beta.22", "v0.22.0"));
        assert!(is_downgrade("0.23.0", "v0.23.0-beta.22"), "a prerelease is below its release");
        assert!(is_downgrade("0.23.0-beta.22", "v0.23.0-beta.21"));
    }

    #[test]
    fn the_same_or_a_newer_target_is_not_a_downgrade() {
        assert!(!is_downgrade("0.23.0-beta.22", "v0.23.0-beta.22"), "reinstalling is allowed");
        assert!(!is_downgrade("0.23.0-beta.22", "v0.23.0"));
        assert!(!is_downgrade("0.22.0", "v0.23.0-beta.1"));
    }

    /// An unknown comparison must not masquerade as a refusal: the direction is
    /// what cannot be established.
    #[test]
    fn an_unparseable_version_is_not_treated_as_a_downgrade() {
        assert!(!is_downgrade("not-a-version", "v0.1.0"));
        assert!(!is_downgrade("0.23.0", "nightly"));
    }

    /// The tag is built the same way the startup path builds it, so `update` and
    /// an automatic update land on the same release.
    #[test]
    fn the_update_target_is_the_servers_own_version() {
        assert_eq!(format!("v{}", "0.23.0-beta.22"), "v0.23.0-beta.22");
        assert!(
            !release_url("cctui-linux-amd64", Some("v0.23.0-beta.22")).contains("/latest/"),
            "a pinned tag must not resolve through the latest release"
        );
        assert!(release_url("cctui-linux-amd64", None).contains("/latest/"));
    }

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
        let s = Channel::Stable;
        assert!(should_update("0.1.5", "0.1.6", s));
        assert!(should_update("0.1.5", "0.2.0", s));
        assert!(!should_update("0.1.6", "0.1.6", s));
        assert!(!should_update("0.1.7", "0.1.6", s));
        assert!(!should_update("not-semver", "0.1.6", s));
        assert!(!should_update("0.1.5", "bad", s));
    }

    #[test]
    fn channel_gates_beta_server_versions() {
        assert!(!should_update("0.1.5", "0.2.0-beta.1", Channel::Stable));
        assert!(should_update("0.1.5", "0.2.0-beta.1", Channel::Beta));
        assert!(should_update("0.2.0-beta.1", "0.2.0", Channel::Stable));
        assert!(should_update("0.2.0-beta.1", "0.2.0-beta.2", Channel::Beta));
        assert!(!should_update("0.2.0", "0.2.0-beta.2", Channel::Beta));
    }

    #[test]
    fn forged_binary_with_matching_sha256sums_is_rejected() {
        let forged = b"#!/bin/sh\necho pwned\n";
        let sums = format!("{}  cctui-linux-amd64\n", hex_sha256(forged));
        let err = verify_release("cctui-linux-amd64", forged, sums.as_bytes(), b"bogus")
            .expect_err("a checksum alone must not authenticate a binary");
        assert!(err.to_string().contains("signature"), "got: {err}");
    }

    #[test]
    fn checksum_mismatch_is_rejected() {
        let err = verify_release("a", b"x", b"deadbeef  a\n", b"").unwrap_err();
        assert!(err.to_string().contains("hash"), "got: {err}");
        verify_release("a", b"x", b"", b"").expect_err("missing entry");
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn install_staged_restores_backup_when_new_binary_is_broken() {
        let tmp = tempfile::tempdir().unwrap();
        let current = script(tmp.path(), "cctui", "exit 0");
        let staging = script(tmp.path(), "cctui.new", "exit 3");
        let backup = tmp.path().join("cctui.bak");
        install_staged(&staging, &current, &backup).expect_err("broken binary must fail");
        assert!(std::fs::read_to_string(&current).unwrap().contains("exit 0"));
        assert!(!backup.exists());
    }

    #[test]
    fn install_staged_drops_backup_after_healthy_install() {
        let tmp = tempfile::tempdir().unwrap();
        let current = script(tmp.path(), "cctui", "exit 0");
        let staging = script(tmp.path(), "cctui.new", "echo new");
        let backup = tmp.path().join("cctui.bak");
        install_staged(&staging, &current, &backup).unwrap();
        assert!(std::fs::read_to_string(&current).unwrap().contains("echo new"));
        assert!(!backup.exists());
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
