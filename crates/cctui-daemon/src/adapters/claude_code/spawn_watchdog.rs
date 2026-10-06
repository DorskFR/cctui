//! Confirms that a dispatched claude-code worker actually started.
//!
//! `claude daemon` accepts a `dispatch` and forks the worker afterwards, so the
//! dispatch reply proves only that the control socket was reachable. A
//! supervisor that is down, or a `claude` binary whose macOS privacy grants an
//! auto-update invalidated, fails after that reply and writes the error to a log
//! file nobody reads. So wait for the job to appear in the roster (or for its
//! `state.json`, which also covers a worker that exits inside the window), and
//! otherwise fail the spawn with the tail of those logs.

use std::fmt::{self, Write as _};
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::socket;
use super::state::StateJson;

/// How long a worker may take to appear in the roster. Generous: a cold
/// supervisor boots in seconds, and a loaded Mac can take a while longer.
pub(super) const DEFAULT_TIMEOUT: Duration = Duration::from_secs(45);

const DEFAULT_POLL: Duration = Duration::from_millis(500);
const TAIL_LINES: usize = 40;
const TAIL_BYTES: u64 = 64 * 1024;
/// Cap for the whole report: the detail travels over the WS and into a DB row.
const REPORT_CAP: usize = 8 * 1024;
const LAN_PROBE_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Deserialize)]
struct RosterPeek {
    #[serde(default)]
    jobs: Vec<RosterJob>,
}

#[derive(Debug, Clone, Deserialize)]
struct RosterJob {
    short: String,
}

#[derive(Debug, Clone)]
pub(super) struct SpawnWatchdog {
    jobs_root: PathBuf,
    timeout: Duration,
    poll: Duration,
    /// Log files whose tail is attached to a stall report, most relevant first.
    logs: Vec<PathBuf>,
    server_url: Option<String>,
}

/// A dispatch the daemon accepted but whose worker never showed up. `Display`
/// is the full diagnostic bundle, so the `CommandResult` error carries it.
#[derive(Debug)]
pub(super) struct SpawnStall {
    short: String,
    waited: Duration,
    hint: Option<String>,
    tails: Vec<(PathBuf, String)>,
}

/// What a TCP connect from cctui-daemon to the server's private address says.
#[derive(Debug)]
pub(super) enum LanProbe {
    Reachable(SocketAddr),
    Denied(SocketAddr, std::io::Error),
}

impl SpawnWatchdog {
    pub(super) fn new(jobs_root: PathBuf) -> Self {
        Self {
            jobs_root,
            timeout: DEFAULT_TIMEOUT,
            poll: DEFAULT_POLL,
            logs: default_logs(),
            server_url: None,
        }
    }

    pub(super) fn with_server(mut self, server_url: Option<String>) -> Self {
        self.server_url = server_url;
        self
    }

    #[cfg(test)]
    pub(super) const fn with_bounds(mut self, timeout: Duration, poll: Duration) -> Self {
        self.timeout = timeout;
        self.poll = poll;
        self
    }

    #[cfg(test)]
    pub(super) fn with_logs(mut self, logs: Vec<PathBuf>) -> Self {
        self.logs = logs;
        self
    }

    /// Poll until `short` is live, or fail with the diagnostic bundle.
    pub(super) async fn confirm(&self, sock: &Path, short: &str) -> Result<(), SpawnStall> {
        let started = tokio::time::Instant::now();
        let deadline = started + self.timeout;
        loop {
            if self.has_started(sock, short, deadline).await {
                return Ok(());
            }
            let now = tokio::time::Instant::now();
            if now >= deadline {
                let lan = match &self.server_url {
                    Some(url) if cfg!(target_os = "macos") => probe_lan(url).await,
                    _ => None,
                };
                return Err(self.stall_with(short, started.elapsed(), lan.as_ref()));
            }
            tokio::time::sleep(self.poll.min(deadline - now)).await;
        }
    }

    /// A wedged daemon must not stretch one roster peek past the deadline.
    async fn has_started(&self, sock: &Path, short: &str, deadline: tokio::time::Instant) -> bool {
        if StateJson::read(&self.jobs_root, short).is_some() {
            return true;
        }
        let req = json!({"proto": 1, "op": "list"});
        let peek = socket::call::<RosterPeek>(sock, &req);
        match tokio::time::timeout_at(deadline, peek).await {
            Ok(Ok(resp)) => resp.jobs.iter().any(|job| job.short == short),
            Ok(Err(err)) => {
                tracing::debug!(%err, %short, "spawn watchdog: roster peek failed");
                false
            }
            Err(_) => false,
        }
    }

    #[cfg(test)]
    fn stall(&self, short: &str, waited: Duration) -> SpawnStall {
        self.stall_with(short, waited, None)
    }

    fn stall_with(&self, short: &str, waited: Duration, lan: Option<&LanProbe>) -> SpawnStall {
        let tails: Vec<(PathBuf, String)> = self
            .logs
            .iter()
            .filter_map(|path| read_tail(path, TAIL_LINES).map(|text| (path.clone(), text)))
            .collect();
        let logged = log_shows_denial(tails.iter().map(|(_, text)| text.as_str()));
        let hint = local_network_hint(cfg!(target_os = "macos"), lan, logged);
        SpawnStall { short: short.to_owned(), waited, hint, tails }
    }
}

impl fmt::Display for SpawnStall {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut out = format!(
            "worker {} never started: the claude daemon accepted the dispatch but the job did not \
             appear in its roster within {}s",
            self.short,
            self.waited.as_secs().max(1),
        );
        if let Some(hint) = &self.hint {
            out.push_str("\nlikely cause: ");
            out.push_str(hint);
        }
        if self.tails.is_empty() {
            out.push_str("\n(no claude-daemon log was readable on this machine)");
        }
        for (path, text) in &self.tails {
            let _ = write!(out, "\n--- {} (last {TAIL_LINES} lines) ---\n", path.display());
            out.push_str(text);
        }
        f.write_str(truncate(&out, REPORT_CAP))
    }
}

/// Where the supervised `claude daemon` and cctui-daemon send their output.
/// macOS: the launchd plists' `StandardErrorPath`. Linux: the unit logs to
/// journald, which a log tail cannot reach, so only cctui's own file is listed.
fn default_logs() -> Vec<PathBuf> {
    let mut logs = Vec::new();
    if cfg!(target_os = "macos") {
        logs.push(PathBuf::from("/tmp/claude-daemon.err.log"));
        logs.push(PathBuf::from("/tmp/claude-daemon.out.log"));
        if let Some(home) = dirs::home_dir() {
            logs.push(home.join("Library").join("Logs").join("cctui-daemon.err.log"));
        }
    }
    logs
}

/// Only the end of the file is read: these logs are never rotated.
fn read_tail(path: &Path, lines: usize) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    file.seek(SeekFrom::Start(len.saturating_sub(TAIL_BYTES))).ok()?;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf).ok()?;
    let lossy = String::from_utf8_lossy(&buf);
    let text: &str = &lossy;
    let text =
        if len > TAIL_BYTES { text.split_once('\n').map_or("", |(_, rest)| rest) } else { text };
    let text = text.trim_end();
    if text.is_empty() {
        return None;
    }
    let all: Vec<&str> = text.lines().collect();
    Some(all[all.len().saturating_sub(lines)..].join("\n"))
}

/// macOS ties Local Network and folder grants to the binary's code signature,
/// so a `claude` auto-update or an ad-hoc-signed cctui-daemon self-update drops
/// them and the worker is denied with no output cctui can see.
fn log_shows_denial<'a>(tails: impl Iterator<Item = &'a str>) -> bool {
    const MARKERS: [&str; 6] = [
        "operation not permitted",
        "no route to host",
        "ehostunreach",
        "enetunreach",
        "local network",
        "nslocalnetwork",
    ];
    tails.flat_map(str::lines).any(|line| {
        let line = line.to_ascii_lowercase();
        MARKERS.iter().any(|m| line.contains(m))
    })
}

const GRANT_FIX: &str = "Grant it in System Settings → Privacy & Security → Local Network, and \
                         re-grant after any binary update: an auto-update or ad-hoc codesign \
                         change invalidates existing grants.";

/// The grant model is macOS-only; elsewhere the hint would mislead.
fn local_network_hint(macos: bool, lan: Option<&LanProbe>, logged: bool) -> Option<String> {
    if !macos {
        return None;
    }
    match (lan, logged) {
        (Some(LanProbe::Denied(addr, err)), _) => Some(format!(
            "macOS Local Network permission denied for cctui-daemon: connecting to the server at \
             {addr} failed ({err}). {GRANT_FIX}"
        )),
        (Some(LanProbe::Reachable(addr)), true) => Some(format!(
            "macOS Local Network permission denied for claude: cctui-daemon reaches the server at \
             {addr}, but the claude-daemon log shows a network or privacy denial. {GRANT_FIX}"
        )),
        (None, true) => Some(format!(
            "macOS denied the claude harness a privacy-gated capability (Local Network or folder \
             access). {GRANT_FIX}"
        )),
        (_, false) => None,
    }
}

/// Only an address on the local network is gated by the macOS grant.
const fn is_lan(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

/// `None` when the server is not on the local network or cannot be resolved:
/// the probe only speaks to the one denial it can see.
pub(super) async fn probe_lan(server_url: &str) -> Option<LanProbe> {
    let url = reqwest::Url::parse(server_url).ok()?;
    let host = url.host_str()?.trim_start_matches('[').trim_end_matches(']').to_owned();
    let port = url.port_or_known_default()?;
    let resolve = tokio::net::lookup_host((host.as_str(), port));
    let addr = tokio::time::timeout(LAN_PROBE_TIMEOUT, resolve)
        .await
        .ok()?
        .ok()?
        .find(|addr| is_lan(addr.ip()))?;
    match tokio::time::timeout(LAN_PROBE_TIMEOUT, tokio::net::TcpStream::connect(addr)).await {
        Ok(Ok(_)) => Some(LanProbe::Reachable(addr)),
        Ok(Err(err)) if is_unreachable(&err) => Some(LanProbe::Denied(addr, err)),
        Ok(Err(err)) => {
            tracing::debug!(%err, %addr, "spawn watchdog: LAN probe failed");
            None
        }
        Err(_) => None,
    }
}

fn is_unreachable(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::HostUnreachable | std::io::ErrorKind::NetworkUnreachable
    )
}

fn truncate(text: &str, cap: usize) -> &str {
    if text.len() <= cap {
        return text;
    }
    let mut end = cap;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn jobs_root_with(short: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let job = dir.path().join(short);
        std::fs::create_dir_all(&job).expect("job dir");
        std::fs::write(job.join("state.json"), br#"{"sessionId":"s-1"}"#).expect("state.json");
        dir
    }

    #[tokio::test]
    async fn a_worker_already_on_disk_confirms_without_a_socket() {
        let dir = jobs_root_with("abcd1234");
        let wd = SpawnWatchdog::new(dir.path().to_path_buf())
            .with_bounds(Duration::from_millis(200), Duration::from_millis(10));
        wd.confirm(Path::new("/nonexistent.sock"), "abcd1234")
            .await
            .expect("state.json presence counts as started");
    }

    /// The whole point of the watchdog: an accepted dispatch whose worker never
    /// appears fails, and the failure carries the claude-daemon log tail.
    #[tokio::test]
    async fn a_worker_that_never_appears_stalls_with_the_log_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("claude-daemon.err.log");
        let mut body = (0..60).fold(String::new(), |mut acc, i| {
            let _ = writeln!(acc, "line {i}");
            acc
        });
        body.push_str("Error: spawn claude ENOENT\n");
        std::fs::write(&log, &body).expect("log");

        let wd = SpawnWatchdog::new(dir.path().join("jobs"))
            .with_bounds(Duration::from_millis(150), Duration::from_millis(10))
            .with_logs(vec![log.clone()]);
        let stall = wd
            .confirm(Path::new("/nonexistent.sock"), "deadbeef")
            .await
            .expect_err("no roster, no state.json — must stall");
        let report = stall.to_string();
        assert!(report.contains("worker deadbeef never started"), "{report}");
        assert!(report.contains("Error: spawn claude ENOENT"), "{report}");
        assert!(report.contains(&log.display().to_string()), "{report}");
        // Tail is bounded: the oldest lines are dropped.
        assert!(report.contains("line 59"), "{report}");
        assert!(!report.contains("line 5\n"), "{report}");
    }

    #[test]
    fn a_missing_log_is_reported_as_such_rather_than_pretending_to_be_empty() {
        let wd = SpawnWatchdog::new(PathBuf::from("/nonexistent/jobs"))
            .with_logs(vec![PathBuf::from("/nonexistent/claude.log")]);
        let report = wd.stall("abcd1234", Duration::from_secs(45)).to_string();
        assert!(report.contains("no claude-daemon log was readable"), "{report}");
    }

    #[test]
    fn the_report_is_capped() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("big.log");
        let body = (0..TAIL_LINES).fold(String::new(), |mut acc, i| {
            let _ = writeln!(acc, "{i}{}", "x".repeat(4096));
            acc
        });
        std::fs::write(&log, body).expect("log");
        let wd = SpawnWatchdog::new(dir.path().join("jobs")).with_logs(vec![log]);
        assert!(wd.stall("abcd1234", Duration::from_secs(45)).to_string().len() <= REPORT_CAP);
    }

    #[test]
    fn tail_keeps_only_the_last_lines_in_order() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("l.log");
        std::fs::write(&log, "a\nb\nc\nd\n").expect("log");
        assert_eq!(read_tail(&log, 2).as_deref(), Some("c\nd"));
        assert_eq!(read_tail(&log, 99).as_deref(), Some("a\nb\nc\nd"));
    }

    #[test]
    fn an_empty_or_absent_log_yields_no_tail() {
        let dir = tempfile::tempdir().expect("tempdir");
        let log = dir.path().join("empty.log");
        std::fs::write(&log, "\n\n").expect("log");
        assert!(read_tail(&log, 10).is_none());
        assert!(read_tail(&dir.path().join("nope.log"), 10).is_none());
    }

    #[test]
    fn the_log_markers_match_the_macos_denial_signatures() {
        assert!(log_shows_denial(["connect EHOSTUNREACH 192.168.1.10:8443"].into_iter()));
        assert!(!log_shows_denial(["worker exited with code 1"].into_iter()));
    }

    fn addr() -> SocketAddr {
        "192.168.1.10:8443".parse().unwrap()
    }

    #[test]
    fn a_denied_probe_names_cctui_daemon_and_the_fix() {
        let denied =
            LanProbe::Denied(addr(), std::io::Error::from(std::io::ErrorKind::HostUnreachable));
        let hint = local_network_hint(true, Some(&denied), false).expect("a denial is a cause");
        assert!(hint.contains("Local Network permission denied for cctui-daemon"), "{hint}");
        assert!(hint.contains("192.168.1.10:8443"), "{hint}");
        assert!(hint.contains("Privacy & Security → Local Network"), "{hint}");
    }

    #[test]
    fn a_reachable_server_with_a_logged_denial_blames_the_worker() {
        let reachable = LanProbe::Reachable(addr());
        let hint = local_network_hint(true, Some(&reachable), true).expect("logged denial");
        assert!(hint.contains("denied for claude"), "{hint}");
        assert!(local_network_hint(true, Some(&reachable), false).is_none());
        assert!(local_network_hint(true, None, true).is_some_and(|h| h.contains("Local Network")));
    }

    #[test]
    fn no_hint_off_macos() {
        let denied =
            LanProbe::Denied(addr(), std::io::Error::from(std::io::ErrorKind::HostUnreachable));
        assert!(local_network_hint(false, Some(&denied), true).is_none());
    }

    #[test]
    fn only_local_network_addresses_are_probed() {
        for lan in ["192.168.1.10", "10.0.0.1", "172.16.4.2", "169.254.1.1", "fd00::1", "fe80::1"] {
            assert!(is_lan(lan.parse().unwrap()), "{lan}");
        }
        for wan in ["8.8.8.8", "127.0.0.1", "2001:db8::1", "::1"] {
            assert!(!is_lan(wan.parse().unwrap()), "{wan}");
        }
    }

    #[test]
    fn unreachable_errors_are_the_denial_signature() {
        assert!(is_unreachable(&std::io::Error::from(std::io::ErrorKind::HostUnreachable)));
        assert!(is_unreachable(&std::io::Error::from(std::io::ErrorKind::NetworkUnreachable)));
        assert!(!is_unreachable(&std::io::Error::from(std::io::ErrorKind::ConnectionRefused)));
    }

    #[tokio::test]
    async fn a_public_or_unparsable_server_is_not_probed() {
        assert!(probe_lan("https://127.0.0.1:1/").await.is_none());
        assert!(probe_lan("not a url").await.is_none());
    }

    /// The probe feeds the stall report, which is the `CommandResult` error the
    /// webui shows as the spawn failure.
    #[test]
    fn a_denied_probe_reaches_the_stall_report_on_macos() {
        let wd = SpawnWatchdog::new(PathBuf::from("/nonexistent/jobs")).with_logs(Vec::new());
        let denied =
            LanProbe::Denied(addr(), std::io::Error::from(std::io::ErrorKind::HostUnreachable));
        let report = wd.stall_with("abcd1234", Duration::from_secs(45), Some(&denied)).to_string();
        assert_eq!(
            report.contains("likely cause: macOS Local Network permission denied"),
            cfg!(target_os = "macos"),
            "{report}"
        );
    }
}
