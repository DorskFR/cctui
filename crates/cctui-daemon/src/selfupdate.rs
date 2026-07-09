//! Self-update routed through the cctui-server (CCT-127).
//!
//! Historically the daemon hit the GitHub API directly (CCT-91). It now
//! goes entirely through the cctui-server, which is the single channel for
//! daemon distribution (the server proxies private-repo release assets when
//! it holds a GitHub PAT — see `cctui-server`'s `routes::manifest`).
//!
//! `cctui-daemon update` is the one-shot path. `cctui-daemon run` also
//! spawns a background ticker that calls [`check_and_apply`] every
//! [`poll_interval`] (default [`DEFAULT_POLL_INTERVAL`]) unless disabled via
//! `--no-auto-update` or `CCTUI_DAEMON_AUTOUPDATE=0`.
//!
//! Steps:
//!   1. `GET {server}/api/v1/manifest/daemon` → the server's version + a
//!      download URL per target.
//!   2. Compare the manifest version against the running `CARGO_PKG_VERSION`.
//!   3. Download the matching `{target}` asset and `SHA256SUMS` from the
//!      server (`/api/v1/daemon/binary/{...}`), verify the checksum,
//!      atomically rename into place, then re-exec.
//!
//! Every request authenticates with the daemon's machine key; the daemon no
//! longer needs a GitHub token of its own. If the server has no PAT it hands
//! back a raw (private, unreachable) GitHub URL for the binary, so the
//! update degrades to a logged no-op until a token is configured server-side.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// Default auto-update poll cadence. Kept short so a pushed release reaches
/// daemons within minutes; override with `CCTUI_DAEMON_AUTOUPDATE_INTERVAL_SECS`.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// Minimum honoured interval — guards against a typo'd env hammering the server.
const MIN_POLL_INTERVAL_SECS: u64 = 30;

/// Resolve the auto-update poll interval, honouring
/// `CCTUI_DAEMON_AUTOUPDATE_INTERVAL_SECS` (seconds, floored at
/// [`MIN_POLL_INTERVAL_SECS`]). Falls back to [`DEFAULT_POLL_INTERVAL`].
#[must_use]
pub fn poll_interval() -> Duration {
    match std::env::var("CCTUI_DAEMON_AUTOUPDATE_INTERVAL_SECS").ok().and_then(|s| s.parse().ok()) {
        Some(secs) if secs >= MIN_POLL_INTERVAL_SECS => Duration::from_secs(secs),
        _ => DEFAULT_POLL_INTERVAL,
    }
}

/// Release-asset basename for this build target, e.g.
/// `cctui-daemon-linux-amd64`. Used for the `SHA256SUMS` lookup (keyed by
/// filename). Empty when no pre-built asset exists for the target.
const fn asset_basename() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "cctui-daemon-linux-amd64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "cctui-daemon-linux-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "cctui-daemon-darwin-arm64"
    } else {
        // No matching pre-built asset; the update path becomes a no-op.
        ""
    }
}

/// The manifest/proxy `{target}` segment for this build target, e.g.
/// `linux-amd64`. Empty when unsupported.
const fn target_name() -> &'static str {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "linux-amd64"
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "linux-arm64"
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "darwin-arm64"
    } else {
        ""
    }
}

/// Server response from `GET /api/v1/manifest/daemon`.
///
/// Shared with the remote-enroll flow (CCT-548), which fetches the manifest
/// with the operator's user token instead of a machine key.
#[derive(Debug, Deserialize)]
pub struct DaemonManifest {
    pub version: String,
    pub assets: Vec<DaemonAsset>,
}

#[derive(Debug, Deserialize)]
pub struct DaemonAsset {
    pub target: String,
    pub url: String,
}

fn manifest_url(server_url: &str) -> String {
    format!("{}/api/v1/manifest/daemon", server_url.trim_end_matches('/'))
}

#[must_use]
pub fn sha256sums_url(server_url: &str) -> String {
    format!("{}/api/v1/daemon/binary/SHA256SUMS", server_url.trim_end_matches('/'))
}

pub fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent(concat!("cctui-daemon/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// Fetch the daemon manifest. `bearer` is a machine key on the self-update
/// path and a user token on the remote-enroll path — the endpoint accepts any
/// authenticated principal.
pub async fn fetch_manifest(
    client: &reqwest::Client,
    server_url: &str,
    bearer: &str,
) -> Result<DaemonManifest> {
    let url = manifest_url(server_url);
    let res =
        client.get(&url).bearer_auth(bearer).header("Accept", "application/json").send().await?;
    if !res.status().is_success() {
        bail!("daemon manifest returned {}", res.status());
    }
    Ok(res.json::<DaemonManifest>().await?)
}

/// Download bytes from a server endpoint, authenticated with `bearer` (a
/// machine key on the self-update path, a user token on remote enroll).
pub async fn download(client: &reqwest::Client, url: &str, bearer: &str) -> Result<Vec<u8>> {
    let res = client
        .get(url)
        .bearer_auth(bearer)
        .header("Accept", "application/octet-stream")
        .send()
        .await?
        .error_for_status()?;
    let bytes = res.bytes().await?;
    Ok(bytes.to_vec())
}

#[must_use]
pub fn parse_sha256sums(text: &str, target: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let name = it.next()?;
        (name == target).then(|| hash.to_owned())
    })
}

#[must_use]
pub fn hex_sha256(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    out.iter().fold(String::with_capacity(out.len() * 2), |mut acc, b| {
        use std::fmt::Write;
        write!(acc, "{b:02x}").expect("formatting to a String is infallible");
        acc
    })
}

fn install_dir() -> Result<PathBuf> {
    std::env::current_exe()?
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow!("current_exe has no parent dir"))
}

fn swap_in_place(target: &Path, bytes: &[u8]) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target.parent().context("target has no parent")?;
    let tmp = dir.join(format!(".{}-new", target.file_name().unwrap().to_string_lossy()));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    std::fs::rename(&tmp, target).with_context(|| format!("rename into {}", target.display()))?;
    Ok(())
}

/// Run one check-and-apply cycle against the cctui-server.
///
/// Returns `Ok(Some(path))` with the resolved install path if the binary was
/// replaced (and the caller should re-exec *that* path — see [`reexec`]);
/// `Ok(None)` if already current or no matching asset; `Err` only on
/// unexpected failures.
#[allow(clippy::cognitive_complexity)]
pub async fn check_and_apply(server_url: &str, machine_key: &str) -> Result<Option<PathBuf>> {
    let asset = asset_basename();
    let target = target_name();
    if asset.is_empty() || target.is_empty() {
        tracing::debug!("no pre-built asset for this target; skipping update");
        return Ok(None);
    }

    let client = client()?;
    let manifest = fetch_manifest(&client, server_url, machine_key).await?;
    let running = env!("CARGO_PKG_VERSION");
    if manifest.version == running {
        tracing::debug!(running, "daemon already on latest release");
        return Ok(None);
    }
    let latest = manifest.version.clone();
    tracing::info!(running, %latest, "newer cctui-daemon release available");

    let binary_url = manifest
        .assets
        .iter()
        .find(|a| a.target == target)
        .map(|a| a.url.clone())
        .ok_or_else(|| anyhow!("manifest {latest} has no asset for target {target}"))?;

    let sums_bytes = download(&client, &sha256sums_url(server_url), machine_key)
        .await
        .context("download SHA256SUMS")?;
    let sums_text = std::str::from_utf8(&sums_bytes).context("SHA256SUMS not UTF-8")?;
    let expected = parse_sha256sums(sums_text, asset)
        .ok_or_else(|| anyhow!("{asset} missing from SHA256SUMS"))?;

    let bin_bytes = download(&client, &binary_url, machine_key).await.context("download binary")?;
    let actual = hex_sha256(&bin_bytes);
    if actual != expected {
        bail!("downloaded {asset} hash {actual} != expected {expected}");
    }

    let dir = install_dir()?;
    let target_path = dir.join("cctui-daemon");
    swap_in_place(&target_path, &bin_bytes)?;
    tracing::info!(version = %latest, target = %target_path.display(), "cctui-daemon binary upgraded");
    Ok(Some(target_path))
}

/// Re-exec into the freshly-swapped binary at `exe`, preserving argv/env.
///
/// `exe` MUST be the resolved install path that [`check_and_apply`] wrote to
/// — NOT `std::env::current_exe()`. After the in-place `rename`, the running
/// process's `/proc/self/exe` symlink points at the now-unlinked old inode,
/// so `current_exe()` can return `".../cctui-daemon (deleted)"`, and
/// `execve`-ing that path fails with `ENOENT` (os error 2) on non-systemd
/// hosts (CCT-152). Re-execing the concrete install path avoids that.
///
/// Caller is responsible for shutting down any stateful work first; we
/// still re-exec on failure to surface the error in the new process if
/// useful.
pub fn reexec(exe: &Path) -> ! {
    let args: Vec<String> = std::env::args().collect();
    let err = std::process::Command::new(exe).args(&args[1..]).exec();
    eprintln!("re-exec failed: {err}");
    std::process::exit(1);
}

/// Spawn the periodic auto-update loop. Cancellation-aware so the
/// supervisor can wind it down cleanly. Applied updates trigger a
/// re-exec via [`reexec`].
pub fn spawn_loop(
    shutdown: CancellationToken,
    server_url: String,
    machine_key: String,
    interval: Duration,
) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        // Skip the immediate first tick — the daemon just started and we
        // do not want the very first action of a fresh process to be a
        // self-replacement (annoying in dev, hides crash loops in prod).
        tick.tick().await;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => return,
                _ = tick.tick() => {}
            }
            match check_and_apply(&server_url, &machine_key).await {
                Ok(Some(exe)) => {
                    // The binary was swapped in place; re-exec so this running
                    // process (incl. the systemd-supervised one — execve keeps
                    // the same PID/cgroup) picks up the new image immediately.
                    // Re-exec the resolved install path, not current_exe() —
                    // see reexec() for why (CCT-152).
                    tracing::info!(exe = %exe.display(), "auto-update applied; re-execing into the new binary");
                    reexec(&exe);
                }
                Ok(None) => {}
                Err(err) => tracing::warn!(%err, "auto-update check failed"),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_sha256sums_picks_matching_line() {
        let sums = "aaa  cctui-darwin-arm64\nbbb  cctui-daemon-linux-amd64\nccc  SHA256SUMS\n";
        assert_eq!(parse_sha256sums(sums, "cctui-daemon-linux-amd64"), Some("bbb".into()));
        assert_eq!(parse_sha256sums(sums, "nope"), None);
    }

    #[test]
    fn hex_sha256_matches_known_vector() {
        // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        let h = hex_sha256(b"");
        assert_eq!(h, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    }

    #[test]
    fn urls_are_built_under_api_v1_without_double_slash() {
        assert_eq!(
            manifest_url("https://cctui.example.com/"),
            "https://cctui.example.com/api/v1/manifest/daemon"
        );
        assert_eq!(
            sha256sums_url("https://cctui.example.com"),
            "https://cctui.example.com/api/v1/daemon/binary/SHA256SUMS"
        );
    }

    #[test]
    fn manifest_deserializes_and_resolves_target_url() {
        let json = r#"{
            "version": "0.3.31",
            "assets": [
                {"target": "linux-amd64", "url": "https://s/api/v1/daemon/binary/linux-amd64"},
                {"target": "darwin-arm64", "url": "https://s/api/v1/daemon/binary/darwin-arm64"}
            ]
        }"#;
        let m: DaemonManifest = serde_json::from_str(json).unwrap();
        assert_eq!(m.version, "0.3.31");
        let url = m.assets.iter().find(|a| a.target == "linux-amd64").map(|a| a.url.as_str());
        assert_eq!(url, Some("https://s/api/v1/daemon/binary/linux-amd64"));
    }
}
