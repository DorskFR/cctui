//! Self-update routed through the cctui-server.
//!
//! Historically the daemon hit the GitHub API directly. It now
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

use crate::counters::{BandwidthCounters, Subsystem};

/// Default auto-update poll cadence. Kept short so a pushed release reaches
/// daemons within minutes; override with `CCTUI_DAEMON_AUTOUPDATE_INTERVAL_SECS`.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_mins(5);

/// Minimum honoured interval — guards against a typo'd env hammering the server.
const MIN_POLL_INTERVAL_SECS: u64 = 30;

static REEXEC_PREP: std::sync::LazyLock<CancellationToken> =
    std::sync::LazyLock::new(CancellationToken::new);

/// Cancelled right before the auto-update re-exec.
///
/// Adapters with stateful children (codex app-servers) subscribe to hibernate
/// them gracefully — execve would otherwise kill them mid-write via CLOEXEC
/// stdin EOF with no teardown at all.
#[must_use]
pub fn reexec_prep() -> CancellationToken {
    REEXEC_PREP.clone()
}

/// How long adapters get between [`reexec_prep`] firing and the execve.
const REEXEC_GRACE: Duration = Duration::from_secs(3);

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
/// Shared with the remote-enroll flow, which fetches the manifest
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

/// Conditional manifest fetch: sends `If-None-Match` when `etag` is set,
/// returns `Ok(None)` on `304` (etag untouched), else stores the response
/// `ETag` in `etag` and returns the parsed manifest.
pub async fn fetch_manifest_conditional(
    client: &reqwest::Client,
    server_url: &str,
    bearer: &str,
    etag: &mut Option<String>,
) -> Result<Option<DaemonManifest>> {
    let url = manifest_url(server_url);
    let mut req = client.get(&url).bearer_auth(bearer).header("Accept", "application/json");
    if let Some(tag) = etag.as_deref() {
        req = req.header(reqwest::header::IF_NONE_MATCH, tag);
    }
    let response = req.send().await?;
    if response.status() == reqwest::StatusCode::NOT_MODIFIED {
        return Ok(None);
    }
    if !response.status().is_success() {
        bail!("daemon manifest returned {}", response.status());
    }
    let new_etag = response
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let manifest = response.json::<DaemonManifest>().await?;
    *etag = new_etag;
    Ok(Some(manifest))
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

/// Swap the new binary into `target`, keeping the previous image as a
/// `.{name}-old` sibling so a failed health check can restore it. Returns the
/// backup path when one was made.
fn swap_in_place(target: &Path, bytes: &[u8]) -> Result<Option<PathBuf>> {
    use std::os::unix::fs::PermissionsExt;
    let dir = target.parent().context("target has no parent")?;
    let name = target.file_name().unwrap().to_string_lossy();
    let tmp = dir.join(format!(".{name}-new"));
    std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
    let backup = dir.join(format!(".{name}-old"));
    let backed_up = target.exists() && std::fs::copy(target, &backup).is_ok();
    std::fs::rename(&tmp, target).with_context(|| format!("rename into {}", target.display()))?;
    Ok(backed_up.then_some(backup))
}

const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(10);

/// A binary that cannot even print `--version` would crashloop under
/// launchd/systemd forever (the updater's first tick is skipped, so a broken
/// image can never heal itself) — gate the re-exec on it.
async fn verify_binary(path: &Path) -> Result<()> {
    let out = tokio::time::timeout(
        HEALTH_CHECK_TIMEOUT,
        tokio::process::Command::new(path).arg("--version").kill_on_drop(true).output(),
    )
    .await
    .map_err(|_| anyhow!("`{} --version` timed out", path.display()))?
    .with_context(|| format!("run `{} --version`", path.display()))?;
    if !out.status.success() {
        bail!("`{} --version` exited {}", path.display(), out.status);
    }
    Ok(())
}

/// Run one check-and-apply cycle against the cctui-server.
///
/// Returns `Ok(Some(path))` with the resolved install path if the binary was
/// replaced (and the caller should re-exec *that* path — see [`reexec`]);
/// `Ok(None)` if already current or no matching asset; `Err` only on
/// unexpected failures.
pub async fn check_and_apply(server_url: &str, machine_key: &str) -> Result<Option<PathBuf>> {
    check_and_apply_with(&client()?, server_url, machine_key, &mut None, &BandwidthCounters::new())
        .await
}

/// [`check_and_apply`] against a caller-owned client + `ETag` cache, so the
/// auto-update loop can pool connections and skip unchanged manifests.
#[allow(clippy::cognitive_complexity)]
pub async fn check_and_apply_with(
    client: &reqwest::Client,
    server_url: &str,
    machine_key: &str,
    etag: &mut Option<String>,
    counters: &BandwidthCounters,
) -> Result<Option<PathBuf>> {
    let asset = asset_basename();
    let target = target_name();
    if asset.is_empty() || target.is_empty() {
        tracing::debug!("no pre-built asset for this target; skipping update");
        return Ok(None);
    }

    let Some(manifest) = fetch_manifest_conditional(client, server_url, machine_key, etag).await?
    else {
        tracing::debug!("daemon manifest unchanged (304); skipping update");
        return Ok(None);
    };
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

    let sums_bytes = download(client, &sha256sums_url(server_url), machine_key)
        .await
        .context("download SHA256SUMS")?;
    counters.add(Subsystem::SelfUpdate, sums_bytes.len() as u64);
    let sums_text = std::str::from_utf8(&sums_bytes).context("SHA256SUMS not UTF-8")?;
    let expected = parse_sha256sums(sums_text, asset)
        .ok_or_else(|| anyhow!("{asset} missing from SHA256SUMS"))?;

    let bin_bytes = download(client, &binary_url, machine_key).await.context("download binary")?;
    counters.add(Subsystem::SelfUpdate, bin_bytes.len() as u64);
    let actual = hex_sha256(&bin_bytes);
    if actual != expected {
        bail!("downloaded {asset} hash {actual} != expected {expected}");
    }

    let dir = install_dir()?;
    let target_path = dir.join("cctui-daemon");
    let backup = swap_in_place(&target_path, &bin_bytes)?;
    if let Err(err) = verify_binary(&target_path).await {
        match backup.map(|b| std::fs::rename(&b, &target_path)) {
            Some(Ok(())) => tracing::error!(
                %err, version = %latest,
                "upgraded binary failed its health check — previous binary restored, \
                 staying on the running image"
            ),
            Some(Err(restore_err)) => tracing::error!(
                %err, %restore_err, version = %latest,
                "upgraded binary failed its health check AND restoring the previous \
                 binary failed — staying on the running image"
            ),
            None => tracing::error!(
                %err, version = %latest,
                "upgraded binary failed its health check and no backup exists — \
                 staying on the running image"
            ),
        }
        return Ok(None);
    }
    let _ = backup.map(std::fs::remove_file);
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
/// hosts. Re-execing the concrete install path avoids that.
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
    counters: BandwidthCounters,
) {
    tokio::spawn(async move {
        let client = match client() {
            Ok(c) => c,
            Err(err) => {
                tracing::error!(%err, "auto-update disabled: could not build HTTP client");
                return;
            }
        };
        let mut etag: Option<String> = None;
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
            match check_and_apply_with(&client, &server_url, &machine_key, &mut etag, &counters)
                .await
            {
                Ok(Some(exe)) => {
                    // The binary was swapped in place; re-exec so this running
                    // process (incl. the systemd-supervised one — execve keeps
                    // the same PID/cgroup) picks up the new image immediately.
                    // Re-exec the resolved install path, not current_exe() —
                    // see reexec() for why.
                    tracing::info!(exe = %exe.display(), "auto-update applied; re-execing into the new binary");
                    REEXEC_PREP.cancel();
                    tokio::time::sleep(REEXEC_GRACE).await;
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

    use std::sync::Arc;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::sync::Mutex;

    /// Accept exactly one connection, capture the raw request, reply with
    /// `response`. A second connect attempt fails (listener dropped) — which
    /// is how the 304 test proves no follow-up download happened.
    async fn serve_once(response: &'static str) -> (String, Arc<Mutex<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let captured = Arc::new(Mutex::new(String::new()));
        let sink = captured.clone();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 2048];
            let n = sock.read(&mut buf).await.unwrap();
            *sink.lock().await = String::from_utf8_lossy(&buf[..n]).into_owned();
            sock.write_all(response.as_bytes()).await.unwrap();
            sock.flush().await.unwrap();
        });
        (format!("http://{addr}"), captured)
    }

    #[tokio::test]
    async fn conditional_fetch_sends_if_none_match_and_treats_304_as_none() {
        let (url, req) = serve_once("HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\n\r\n").await;
        let mut etag = Some("\"v1\"".to_string());
        let got = fetch_manifest_conditional(&client().unwrap(), &url, "key", &mut etag).await;
        assert!(matches!(got, Ok(None)));
        assert_eq!(etag.as_deref(), Some("\"v1\""));
        assert!(req.lock().await.to_lowercase().contains("if-none-match: \"v1\""));
    }

    #[tokio::test]
    async fn conditional_fetch_parses_200_and_stores_etag() {
        let response = concat!(
            "HTTP/1.1 200 OK\r\nETag: \"v2\"\r\nContent-Type: application/json\r\n",
            "Content-Length: 31\r\n\r\n{\"version\":\"9.9.9\",\"assets\":[]}",
        );
        let (url, _req) = serve_once(response).await;
        let mut etag = None;
        let m = fetch_manifest_conditional(&client().unwrap(), &url, "key", &mut etag)
            .await
            .unwrap()
            .expect("200 yields a manifest");
        assert_eq!(m.version, "9.9.9");
        assert_eq!(etag.as_deref(), Some("\"v2\""));
    }

    #[tokio::test]
    async fn check_and_apply_skips_everything_on_304() {
        // The mock serves a single 304; a version compare or download would
        // need a second request and thus fail. Ok(None) proves neither ran.
        let (url, _req) = serve_once("HTTP/1.1 304 Not Modified\r\nETag: \"v1\"\r\n\r\n").await;
        let mut etag = Some("\"v1\"".to_string());
        let out = check_and_apply_with(
            &client().unwrap(),
            &url,
            "key",
            &mut etag,
            &BandwidthCounters::new(),
        )
        .await;
        assert!(matches!(out, Ok(None)));
    }

    fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn verify_binary_accepts_a_healthy_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let ok = write_script(tmp.path(), "ok", "exit 0");
        verify_binary(&ok).await.expect("clean --version passes the gate");
    }

    #[tokio::test]
    async fn verify_binary_rejects_broken_binaries() {
        let tmp = tempfile::tempdir().unwrap();
        let bad = write_script(tmp.path(), "bad", "exit 3");
        let err = verify_binary(&bad).await.expect_err("non-zero exit must fail the gate");
        assert!(err.to_string().contains("exited"), "got: {err}");
        verify_binary(&tmp.path().join("missing")).await.expect_err("unexecutable must fail");
    }

    #[test]
    fn swap_in_place_keeps_a_restorable_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("cctui-daemon");
        std::fs::write(&target, b"old").unwrap();
        let backup =
            swap_in_place(&target, b"new").unwrap().expect("existing target must be backed up");
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        assert_eq!(backup, tmp.path().join(".cctui-daemon-old"));
    }

    #[test]
    fn swap_in_place_first_install_has_no_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("cctui-daemon");
        assert!(swap_in_place(&target, b"new").unwrap().is_none());
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
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
