//! Probe whether codex's bubblewrap sandbox can actually start on this host.
//!
//! `codex doctor` reports the sandbox as healthy on a host where every
//! sandboxed command fails, and the only signal codex emits — a connection-level
//! `configWarning` — arrives before any thread exists, so on a shared
//! app-server it reaches no session. The probe runs a sandboxed `/bin/true`
//! instead and classifies the failure.
//!
//! The result is cached against the `realpath` of the codex binary: it moves on
//! every codex release, and a new binary is exactly when the answer can change.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use cctui_proto::harness::CodexSandbox;

const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Codex's own markers for "the host refused a user namespace". Anything else
/// non-zero is [`CodexSandbox::Unknown`] — guessing at an unfamiliar bwrap
/// failure would put wrong fix instructions in front of the user.
const USERNS_MARKERS: &[&str] = &[
    "loopback: Failed RTM_NEWADDR",
    "loopback: Failed RTM_NEWLINK",
    "setting up uid map: Permission denied",
    "No permissions to create a new namespace",
];

#[derive(Debug, Clone, Default)]
pub struct ProbeOutput {
    pub ok: bool,
    pub stderr: String,
}

/// The one side-effecting step, behind a trait so tests never need a real codex.
#[async_trait::async_trait]
pub trait SandboxRunner: Send + Sync {
    async fn run(&self, bin: &str) -> std::io::Result<ProbeOutput>;
}

pub struct RealRunner;

#[async_trait::async_trait]
impl SandboxRunner for RealRunner {
    async fn run(&self, bin: &str) -> std::io::Result<ProbeOutput> {
        let dir = std::env::temp_dir();
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("sandbox")
            .arg("-c")
            .arg(r#"sandbox_mode="workspace-write""#)
            .arg("-C")
            .arg(&dir)
            .arg("--")
            .arg("/bin/true")
            .current_dir(&dir)
            .env("PATH", crate::childenv::child_path())
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        crate::childenv::ScrubChildEnv::scrub_child_env(&mut cmd);
        let output = tokio::time::timeout(PROBE_TIMEOUT, cmd.output())
            .await
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "probe timed out"))??;
        Ok(ProbeOutput {
            ok: output.status.success(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// The userns marker `text` contains, if any. Shared with the per-session
/// belt-and-braces check so one list governs both.
#[must_use]
pub fn userns_marker(text: &str) -> Option<&'static str> {
    USERNS_MARKERS.iter().copied().find(|m| text.contains(m))
}

/// Whether a command's output is a bwrap failure this host's policy explains.
/// Requires the `bwrap:` prefix as well as a marker: a command whose own output
/// merely quotes one of these strings must not raise the alarm.
#[must_use]
pub fn bwrap_failure(output: &str) -> Option<&'static str> {
    let first = output.trim_start().lines().next().unwrap_or_default();
    if !first.starts_with("bwrap:") {
        return None;
    }
    userns_marker(output)
}

fn first_useful_line(stderr: &str) -> String {
    stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("codex sandbox probe failed with no output")
        .to_owned()
}

#[must_use]
pub fn classify(out: &ProbeOutput) -> CodexSandbox {
    if out.ok {
        return CodexSandbox::Ok;
    }
    let detail = first_useful_line(&out.stderr);
    if userns_marker(&out.stderr).is_some() {
        CodexSandbox::UsernsDenied { detail }
    } else {
        CodexSandbox::Unknown { detail }
    }
}

pub async fn probe_with(bin: &str, runner: &dyn SandboxRunner) -> CodexSandbox {
    match runner.run(bin).await {
        Ok(out) => classify(&out),
        Err(err) => CodexSandbox::Unknown { detail: format!("could not run the probe: {err}") },
    }
}

/// `realpath` of `bin`, resolved through `PATH` when it is a bare name. Falls
/// back to the name itself so an unresolvable binary still caches under a
/// stable key.
fn resolved_bin(bin: &str) -> PathBuf {
    let direct = PathBuf::from(bin);
    let candidate = if direct.is_absolute() || bin.contains('/') {
        direct
    } else {
        crate::childenv::child_path()
            .split(':')
            .map(|dir| PathBuf::from(dir).join(bin))
            .find(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from(bin))
    };
    std::fs::canonicalize(&candidate).unwrap_or(candidate)
}

fn cache() -> &'static Mutex<Option<(PathBuf, CodexSandbox)>> {
    static CACHE: std::sync::OnceLock<Mutex<Option<(PathBuf, CodexSandbox)>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// The last probe result, for the heartbeat. `None` before the first probe.
#[must_use]
pub fn last() -> Option<CodexSandbox> {
    cache().lock().ok().and_then(|g| g.as_ref().map(|(_, v)| v.clone()))
}

/// Probe unless `bin` resolves to the same binary as the cached result. Returns
/// the effective verdict either way.
pub async fn probe_cached(bin: &str, runner: &dyn SandboxRunner) -> CodexSandbox {
    let key = resolved_bin(bin);
    if let Ok(guard) = cache().lock()
        && let Some((cached_key, verdict)) = guard.as_ref()
        && *cached_key == key
    {
        return verdict.clone();
    }
    let verdict = probe_with(bin, runner).await;
    if let Ok(mut guard) = cache().lock() {
        *guard = Some((key, verdict.clone()));
    }
    verdict
}

/// Probe with the real runner and log the verdict.
pub async fn refresh(bin: &str) -> CodexSandbox {
    let verdict = probe_cached(bin, &RealRunner).await;
    match &verdict {
        CodexSandbox::Ok => tracing::debug!(bin, "codex sandbox probe ok"),
        CodexSandbox::UsernsDenied { detail } => {
            tracing::warn!(bin, %detail, "codex sandbox cannot start: user namespaces denied");
        }
        CodexSandbox::Unknown { detail } => {
            tracing::warn!(bin, %detail, "codex sandbox probe failed");
        }
    }
    verdict
}

#[cfg(test)]
pub(crate) fn reset_cache_for_test() {
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    struct Fake {
        out: ProbeOutput,
        calls: AtomicUsize,
    }

    impl Fake {
        fn failing(stderr: &str) -> Self {
            Self {
                out: ProbeOutput { ok: false, stderr: stderr.to_owned() },
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl SandboxRunner for Fake {
        async fn run(&self, _bin: &str) -> std::io::Result<ProbeOutput> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.out.clone())
        }
    }

    struct Broken;

    #[async_trait::async_trait]
    impl SandboxRunner for Broken {
        async fn run(&self, _bin: &str) -> std::io::Result<ProbeOutput> {
            Err(std::io::Error::new(std::io::ErrorKind::NotFound, "no such file"))
        }
    }

    const LOOPBACK: &str = "bwrap: loopback: Failed RTM_NEWADDR: Operation not permitted";

    #[test]
    fn a_successful_probe_is_ok() {
        assert!(classify(&ProbeOutput { ok: true, stderr: String::new() }).is_ok());
    }

    #[test]
    fn every_marker_codex_knows_classifies_as_userns_denied() {
        for marker in USERNS_MARKERS {
            let out = ProbeOutput { ok: false, stderr: format!("bwrap: {marker}: whatever") };
            match classify(&out) {
                CodexSandbox::UsernsDenied { detail } => assert!(detail.contains(marker)),
                other => panic!("{marker} classified as {other:?}"),
            }
        }
    }

    /// An unfamiliar failure must not claim the AppArmor fix applies.
    #[test]
    fn an_unrecognized_failure_is_unknown_and_keeps_its_first_line() {
        let out = ProbeOutput {
            ok: false,
            stderr: "\n  bwrap: Can't find source path /nope\nsecond line\n".to_owned(),
        };
        match classify(&out) {
            CodexSandbox::Unknown { detail } => {
                assert_eq!(detail, "bwrap: Can't find source path /nope")
            }
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn a_failure_with_no_output_still_carries_a_detail() {
        match classify(&ProbeOutput { ok: false, stderr: "  \n".to_owned() }) {
            CodexSandbox::Unknown { detail } => assert!(!detail.is_empty()),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_runner_that_cannot_start_is_unknown_rather_than_ok() {
        let verdict = probe_with("codex", &Broken).await;
        assert!(!verdict.is_ok());
        assert!(verdict.detail().is_some_and(|d| d.contains("could not run the probe")));
    }

    #[tokio::test]
    async fn the_same_binary_is_probed_once_and_a_new_one_re_probes() {
        reset_cache_for_test();
        let fake = Fake::failing(LOOPBACK);
        let bin = std::env::current_exe().expect("test binary path");
        let bin = bin.to_string_lossy().into_owned();

        assert!(matches!(probe_cached(&bin, &fake).await, CodexSandbox::UsernsDenied { .. }));
        probe_cached(&bin, &fake).await;
        assert_eq!(fake.calls.load(Ordering::SeqCst), 1, "a cached verdict must not re-probe");
        assert!(last().is_some_and(|v| !v.is_ok()));

        let moved = Fake {
            out: ProbeOutput { ok: true, ..ProbeOutput::default() },
            calls: AtomicUsize::new(0),
        };
        assert!(probe_cached("/nonexistent/codex-0.154.0", &moved).await.is_ok());
        assert_eq!(moved.calls.load(Ordering::SeqCst), 1, "a moved binary must re-probe");
        reset_cache_for_test();
    }

    /// A command whose own output merely quotes a marker must not trip the
    /// per-session notice; only a real `bwrap:` failure line does.
    #[test]
    fn only_a_bwrap_prefixed_output_counts_as_a_sandbox_failure() {
        assert!(bwrap_failure(LOOPBACK).is_some());
        assert!(bwrap_failure(&format!("  \n{LOOPBACK}")).is_some());
        assert!(bwrap_failure("grep: loopback: Failed RTM_NEWADDR").is_none());
        assert!(bwrap_failure("bwrap: some other complaint").is_none());
        assert!(bwrap_failure("").is_none());
    }
}
