//! Discover the `claude daemon` control socket.
//!
//! Path scheme: `/tmp/cc-daemon-<uid>/<hash>/control.sock`. `<hash>` is a
//! `$HOME`-derived directory name we do not need to reverse — we enumerate
//! `/tmp/cc-daemon-<uid>/` and pick the subdirectory that contains a *live*
//! `control.sock`. Tests override the base via [`Discovery::with_base`].
//!
//! A Unix socket *file* can outlive the daemon that created it: when `claude`
//! auto-updates (or the on-demand daemon crashes / is `kill -9`'d) the inode
//! stays on disk while the listener is gone. `exists()` still returns `true`,
//! so a naive picker selects a **dead** socket and every connect either gets
//! ECONNREFUSED or hangs half-open — cctui then "stops receiving messages" and
//! dispatch fails with "daemon offline" until the user wakes it by hand.
//! [`Discovery::locate_live`] guards against this by connecting and
//! pinging each candidate, returning only a reachable one and reaping the
//! corpse (a hard-refused socket file) so it stops shadowing discovery.
//!
//! We never signal or kill a `claude daemon` — it is not our process and an
//! interactive `claude` may share it. The remedy is socket *selection* only.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

/// How long a single liveness probe (connect + `list` ping) may take before we
/// treat the candidate as unreachable. A live daemon answers a localhost UDS
/// `list` in well under this; the generous bound avoids falsely skipping a
/// momentarily-busy daemon (which we must NOT reap — see [`Probe`]).
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// Outcome of probing one candidate `control.sock`.
enum Probe {
    /// Connected and got a response line — a live daemon is listening.
    Live,
    /// Hard `ECONNREFUSED`: the socket file exists but nothing is listening
    /// (the daemon that created it is gone). Safe to unlink the corpse.
    Refused,
    /// Connected but didn't answer in time, or any other connect error
    /// (ENOENT race, non-socket file, …). Skip it but do NOT reap — a slow
    /// daemon must survive a single sluggish probe.
    Unreachable,
}

#[derive(Debug, Clone)]
pub struct Discovery {
    base: PathBuf,
}

impl Discovery {
    /// Default: `/tmp/cc-daemon-<uid>` for the current process.
    #[must_use]
    pub fn for_current_user() -> Self {
        let uid = rustix::process::getuid().as_raw();
        Self::with_base(PathBuf::from(format!("/tmp/cc-daemon-{uid}")))
    }

    #[must_use]
    pub const fn with_base(base: PathBuf) -> Self {
        Self { base }
    }

    /// Candidate `<base>/<hash>/control.sock` paths whose file exists, in
    /// deterministic (lexicographically-sorted hash dir) order.
    fn candidates(&self) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&self.base)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.is_dir())
            .collect();
        dirs.sort();
        dirs.into_iter().map(|d| d.join("control.sock")).filter(|c| c.exists()).collect()
    }

    /// The candidate socket paths, for observability (session diagnose).
    /// Same enumeration [`Self::locate_live`] walks; no probing.
    pub(super) fn candidate_paths(&self) -> Vec<PathBuf> {
        self.candidates()
    }

    /// Returns the first `<base>/<hash>/control.sock` that exists by `exists()`
    /// alone, ignoring liveness. Cheap and synchronous; prefer
    /// [`Discovery::locate_live`] on the poll/dispatch paths where a dead
    /// socket would silently wedge the adapter.
    // Used by the ignored live integration tests in `socket.rs`; the cheap
    // sync path is kept available, hence the allow on non-test builds.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn locate(&self) -> Option<PathBuf> {
        self.candidates().into_iter().next()
    }

    /// Returns the first candidate whose daemon actually answers a `list`
    /// ping. A dead/stale socket is skipped (and, on hard `ECONNREFUSED`,
    /// best-effort unlinked so it stops shadowing future discovery). Returns
    /// `None` when no candidate is reachable — letting the caller's kickstart
    /// self-heal fire to bring a fresh daemon up.
    pub async fn locate_live(&self) -> Option<PathBuf> {
        for candidate in self.candidates() {
            match probe(&candidate).await {
                Probe::Live => return Some(candidate),
                Probe::Refused => {
                    // Connect to a *regular* file also yields ECONNREFUSED, so
                    // only reap when the path is genuinely a socket inode — we
                    // never want to clobber a non-socket that happens to sit at
                    // this name.
                    if is_socket(&candidate) {
                        tracing::info!(
                            sock = %candidate.display(),
                            "reaping stale claude control.sock (connection refused — daemon gone)"
                        );
                        // Best-effort: a failed unlink just means we'll skip it
                        // again next time, never worse than the status quo.
                        let _ = std::fs::remove_file(&candidate);
                    }
                }
                Probe::Unreachable => {
                    tracing::debug!(
                        sock = %candidate.display(),
                        "claude control.sock unreachable (skipping; not reaping)"
                    );
                }
            }
        }
        None
    }
}

/// Whether `path` is a Unix-domain socket inode (vs. a regular file we must
/// never reap). Returns false on any stat error.
fn is_socket(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_socket())
}

/// Connect to `socket` and send one `list` ping, classifying the result. Never
/// reaps — the caller decides based on the [`Probe`] verdict.
async fn probe(socket: &Path) -> Probe {
    match tokio::time::timeout(PROBE_TIMEOUT, probe_inner(socket)).await {
        Ok(Ok(())) => Probe::Live,
        Ok(Err(err)) if err.kind() == ErrorKind::ConnectionRefused => Probe::Refused,
        Ok(Err(_)) | Err(_) => Probe::Unreachable,
    }
}

/// One connect + `{"proto":1,"op":"list"}` round-trip. The write half is held
/// open until the response line is read: the daemon drops a request as stale
/// the moment it sees EOF on the read side, so half-closing early can void the
/// op. `list` is read-only, so probing it has no side effects.
async fn probe_inner(socket: &Path) -> std::io::Result<()> {
    let stream = UnixStream::connect(socket).await?;
    let (read_half, mut write_half) = stream.into_split();
    write_half.write_all(b"{\"proto\":1,\"op\":\"list\"}\n").await?;
    write_half.flush().await?;
    let mut reader = BufReader::new(read_half);
    let mut buf = String::new();
    let n = reader.read_line(&mut buf).await?;
    drop(write_half);
    if n == 0 {
        return Err(std::io::Error::new(ErrorKind::UnexpectedEof, "no response line"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixListener;

    #[test]
    fn locate_returns_none_when_base_missing() {
        let d = Discovery::with_base(PathBuf::from("/tmp/cctui-test-missing-xyz"));
        assert!(d.locate().is_none());
    }

    #[test]
    fn locate_finds_first_control_sock() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let hash_dir = base.join("06032065");
        std::fs::create_dir(&hash_dir).unwrap();
        // Need a real file at control.sock — a UDS bind is not necessary
        // for locate(); it only checks existence.
        std::fs::write(hash_dir.join("control.sock"), b"").unwrap();
        let d = Discovery::with_base(base);
        let p = d.locate().unwrap();
        assert!(p.ends_with("06032065/control.sock"));
    }

    #[test]
    fn locate_skips_dirs_without_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        std::fs::create_dir(base.join("empty")).unwrap();
        let good = base.join("00abcdef");
        std::fs::create_dir(&good).unwrap();
        std::fs::write(good.join("control.sock"), b"").unwrap();
        let d = Discovery::with_base(base);
        let p = d.locate().unwrap();
        assert!(p.ends_with("00abcdef/control.sock"));
    }

    /// Spawn a minimal `claude daemon`-shaped responder at `path`: accept
    /// connections, read the request line, reply `{"ok":true,"op":"list"}`.
    /// The detached task owns the listener and loops forever, so the socket
    /// stays live for the test's duration (and answers repeated probes).
    fn spawn_live_socket(path: &Path) {
        let listener = UnixListener::bind(path).expect("bind live socket");
        tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { return };
                tokio::spawn(async move {
                    let (read_half, mut write_half) = stream.into_split();
                    let mut reader = BufReader::new(read_half);
                    let mut line = String::new();
                    let _ = reader.read_line(&mut line).await;
                    let _ =
                        write_half.write_all(b"{\"ok\":true,\"op\":\"list\",\"jobs\":[]}\n").await;
                    let _ = write_half.flush().await;
                });
            }
        });
    }

    #[tokio::test]
    async fn locate_live_prefers_reachable_over_stale() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();

        // Lexicographically FIRST dir holds a dead socket (bound then the
        // listener dropped — the file lingers, connect → ECONNREFUSED).
        let dead_dir = base.join("00000000");
        std::fs::create_dir(&dead_dir).unwrap();
        let dead = dead_dir.join("control.sock");
        drop(UnixListener::bind(&dead).unwrap());
        assert!(dead.exists());

        // A LATER dir holds a live socket; locate_live must skip the dead one.
        let live_dir = base.join("11111111");
        std::fs::create_dir(&live_dir).unwrap();
        let live = live_dir.join("control.sock");
        spawn_live_socket(&live);

        let d = Discovery::with_base(base);
        let found = d.locate_live().await.expect("should find the live socket");
        assert_eq!(found, live, "must prefer the reachable socket over the stale first one");
        // The corpse was reaped on ECONNREFUSED so it stops shadowing discovery.
        assert!(!dead.exists(), "stale refused socket should be unlinked");
    }

    #[tokio::test]
    async fn locate_live_none_for_lone_dead_socket_and_reaps_it() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let dir = base.join("deadbeef");
        std::fs::create_dir(&dir).unwrap();
        let dead = dir.join("control.sock");
        drop(UnixListener::bind(&dead).unwrap());

        let d = Discovery::with_base(base);
        assert!(
            d.locate_live().await.is_none(),
            "a lone dead socket yields None (kickstart heals)"
        );
        assert!(!dead.exists(), "refused socket should be reaped");
    }

    #[tokio::test]
    async fn locate_live_does_not_reap_a_non_socket_file() {
        // A regular file (or any non-refused error) is unreachable but must NOT
        // be reaped — we only unlink on a hard ECONNREFUSED.
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path().to_path_buf();
        let dir = base.join("00abcdef");
        std::fs::create_dir(&dir).unwrap();
        let file = dir.join("control.sock");
        std::fs::write(&file, b"not a socket").unwrap();

        let d = Discovery::with_base(base);
        assert!(d.locate_live().await.is_none());
        assert!(file.exists(), "a non-refused candidate must not be reaped");
    }
}
