//! Confirms that a dispatched claude-code worker actually started.
//!
//! `claude daemon` accepts a `dispatch` and forks the worker afterwards, so the
//! dispatch reply proves only that the control socket was reachable. A
//! supervisor that is down, or a `claude` binary whose macOS privacy grants an
//! auto-update invalidated, fails after that reply and writes the error to a log
//! file nobody reads. So wait for the job to appear in the roster (or for its
//! `state.json`, which also covers a worker that exits inside the window), and
//! otherwise fail the spawn with the tail of those logs.

use std::fmt;
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
}

/// A dispatch the daemon accepted but whose worker never showed up. `Display`
/// is the full diagnostic bundle, so the `CommandResult` error carries it.
#[derive(Debug)]
pub(super) struct SpawnStall {
    short: String,
    waited: Duration,
    hint: Option<&'static str>,
    tails: Vec<(PathBuf, String)>,
}

impl SpawnWatchdog {
    pub(super) fn new(jobs_root: PathBuf) -> Self {
        Self { jobs_root, timeout: DEFAULT_TIMEOUT, poll: DEFAULT_POLL, logs: default_logs() }
    }

    #[cfg(test)]
    pub(super) fn with_bounds(mut self, timeout: Duration, poll: Duration) -> Self {
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
                return Err(self.stall(short, started.elapsed()));
            }
            tokio::time::sleep(self.poll.min(deadline - now)).await;
        }
    }

    /// A wedged daemon must not stretch one roster peek past the deadline.
    async fn has_started(&self, sock: &Path, short: &str, deadline: tokio::time::Instant) -> bool {
        if StateJson::read(&self.jobs_root, short).is_some() {
            return true;
        }
        let peek = socket::call::<RosterPeek>(sock, &json!({"proto": 1, "op": "list"}));
        match tokio::time::timeout_at(deadline, peek).await {
            Ok(Ok(resp)) => resp.jobs.iter().any(|job| job.short == short),
            Ok(Err(err)) => {
                tracing::debug!(%err, %short, "spawn watchdog: roster peek failed");
                false
            }
            Err(_) => false,
        }
    }

    fn stall(&self, short: &str, waited: Duration) -> SpawnStall {
        let tails: Vec<(PathBuf, String)> = self
            .logs
            .iter()
            .filter_map(|path| read_tail(path, TAIL_LINES).map(|text| (path.clone(), text)))
            .collect();
        let hint = classify(tails.iter().map(|(_, text)| text.as_str()));
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
        if let Some(hint) = self.hint {
            out.push_str("\nlikely cause: ");
            out.push_str(hint);
        }
        if self.tails.is_empty() {
            out.push_str("\n(no claude-daemon log was readable on this machine)");
        }
        for (path, text) in &self.tails {
            out.push_str(&format!("\n--- {} (last {TAIL_LINES} lines) ---\n", path.display()));
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
    let text = String::from_utf8_lossy(&buf);
    let text =
        if len > TAIL_BYTES { text.split_once('\n').map_or("", |(_, rest)| rest) } else { &text };
    let text = text.trim_end();
    if text.is_empty() {
        return None;
    }
    let kept: Vec<&str> = text.lines().rev().take(lines).collect();
    Some(kept.into_iter().rev().collect::<Vec<_>>().join("\n"))
}

/// Name the macOS privacy denial when a log tail shows its signature. macOS
/// ties Local Network and folder grants to the binary's code signature, so a
/// `claude` auto-update or an ad-hoc-signed cctui-daemon self-update drops them
/// and the worker is denied with no output cctui can see.
fn classify<'a>(tails: impl Iterator<Item = &'a str>) -> Option<&'static str> {
    const MARKERS: [&str; 6] = [
        "operation not permitted",
        "no route to host",
        "ehostunreach",
        "enetunreach",
        "local network",
        "nslocalnetwork",
    ];
    if !cfg!(target_os = "macos") {
        return None;
    }
    let hit = tails.flat_map(str::lines).any(|line| {
        let line = line.to_ascii_lowercase();
        MARKERS.iter().any(|m| line.contains(m))
    });
    hit.then_some(
        "macOS denied the claude harness a privacy-gated capability (Local Network or folder \
         access). Grant it in System Settings → Privacy & Security → Local Network, and re-grant \
         after any binary update: an auto-update or ad-hoc codesign change invalidates existing \
         grants.",
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
        let mut body: String = (0..60).map(|i| format!("line {i}\n")).collect();
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
        let body: String = (0..TAIL_LINES).map(|i| format!("{i}{}\n", "x".repeat(4096))).collect();
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
    fn the_local_network_classifier_matches_the_macos_denial_signatures() {
        let denial = classify(["connect EHOSTUNREACH 192.168.1.10:8443"].into_iter());
        let benign = classify(["worker exited with code 1"].into_iter());
        if cfg!(target_os = "macos") {
            assert!(denial.expect("classified").contains("Local Network"));
            assert!(benign.is_none());
        } else {
            // The grant model is macOS-only; elsewhere the hint would mislead.
            assert!(denial.is_none());
            assert!(benign.is_none());
        }
    }
}
