use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::json;

use super::super::session_registry;
use super::{Driver, socket};

const PROBE_INTERVAL: Duration = Duration::from_secs(20);
const RECHECK_INTERVAL: Duration = Duration::from_secs(3);
const MISSES_TO_CONFIRM: u8 = 2;
const HAS_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PROBES_PER_PASS: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Verdict {
    Alive,
    Gone,
    Unknown,
}

#[derive(Debug)]
struct Worker {
    local_id: String,
    started_at: Option<i64>,
    last_probe: Option<Instant>,
    misses: u8,
    confirmed: bool,
}

/// Authoritative `has` liveness for the fleet workers `list` reports live, so
/// a worker whose process is gone leaves `active` even when `list` keeps it
/// unflagged or cannot be read.
#[derive(Debug, Default)]
pub(super) struct Liveness {
    workers: HashMap<String, Worker>,
    /// The socket the roster was last listed from: the only one whose `has`
    /// answer speaks for these workers.
    host: Option<PathBuf>,
}

impl Liveness {
    pub(super) fn set_host(&mut self, sock: &Path) {
        if self.host.as_deref() != Some(sock) {
            self.host = Some(sock.to_path_buf());
            for w in self.workers.values_mut() {
                w.misses = 0;
                w.confirmed = false;
            }
        }
    }

    pub(super) fn hosts(&self, sock: &Path) -> bool {
        self.host.as_deref() == Some(sock)
    }

    /// A worker `list` reports live. A changed `started_at` is a respawn and
    /// clears any earlier verdict.
    pub(super) fn observe(&mut self, short: &str, local_id: &str, started_at: Option<i64>) {
        let w = self.workers.entry(short.to_owned()).or_insert_with(|| Worker {
            local_id: local_id.to_owned(),
            started_at,
            last_probe: None,
            misses: 0,
            confirmed: false,
        });
        local_id.clone_into(&mut w.local_id);
        if w.started_at != started_at {
            w.started_at = started_at;
            w.misses = 0;
            w.confirmed = false;
        }
    }

    pub(super) fn forget(&mut self, short: &str) {
        self.workers.remove(short);
    }

    pub(super) fn retain(&mut self, roster: &HashSet<String>) {
        self.workers.retain(|short, _| roster.contains(short));
    }

    pub(super) fn is_confirmed_dead(&self, short: &str) -> bool {
        self.workers.get(short).is_some_and(|w| w.confirmed)
    }

    pub(super) fn due(&self, now: Instant) -> Vec<String> {
        let mut due: Vec<(&String, Option<Instant>)> = self
            .workers
            .iter()
            .filter(|(_, w)| {
                let spacing = if w.misses > 0 { RECHECK_INTERVAL } else { PROBE_INTERVAL };
                w.last_probe.is_none_or(|at| now.saturating_duration_since(at) >= spacing)
            })
            .map(|(short, w)| (short, w.last_probe))
            .collect();
        due.sort_by_key(|(_, at)| *at);
        due.into_iter().take(MAX_PROBES_PER_PASS).map(|(s, _)| s.clone()).collect()
    }

    /// Record one probe. Returns the worker's `local_id` when this answer is
    /// the one that confirms its death.
    pub(super) fn record(&mut self, short: &str, verdict: Verdict, now: Instant) -> Option<String> {
        let w = self.workers.get_mut(short)?;
        w.last_probe = Some(now);
        match verdict {
            Verdict::Alive => {
                w.misses = 0;
                w.confirmed = false;
                None
            }
            Verdict::Unknown => None,
            Verdict::Gone => {
                w.misses = w.misses.saturating_add(1);
                if w.confirmed || w.misses < MISSES_TO_CONFIRM {
                    return None;
                }
                w.confirmed = true;
                Some(w.local_id.clone())
            }
        }
    }
}

/// `has` for `short`, read strictly: only an explicit `alive:false` is gone,
/// and the CLI session registry holding a verifiable live pid overrides it.
pub(super) async fn probe(sock: &Path, short: &str) -> Verdict {
    let request = json!({"proto":1,"op":"has","short":short});
    let Ok(resp) = socket::one_shot_within(sock, &request, HAS_TIMEOUT).await else {
        return Verdict::Unknown;
    };
    match resp.get("alive").and_then(serde_json::Value::as_bool) {
        Some(true) => Verdict::Alive,
        Some(false) => {
            let registered_live = session_registry::default_dir()
                .is_some_and(|dir| session_registry::live_pid_for_job(&dir, short).is_some());
            if registered_live { Verdict::Alive } else { Verdict::Gone }
        }
        None => Verdict::Unknown,
    }
}

impl Driver {
    /// Probe the due fleet workers on `sock` and hibernate any whose death this
    /// pass confirms: a probe is not proof enough to end a session and delete
    /// its files, and a hibernated one still cold-resumes on the next reply.
    pub(super) async fn probe_liveness(&mut self, sock: &Path) {
        if !self.liveness.hosts(sock) {
            return;
        }
        for short in self.liveness.due(Instant::now()) {
            if self.foreign_shorts.contains(&short) {
                self.liveness.forget(&short);
                continue;
            }
            let verdict = probe(sock, &short).await;
            if let Some(local_id) = self.liveness.record(&short, verdict, Instant::now()) {
                tracing::warn!(%short, %local_id, "claude reports the worker gone; marking it dead");
                self.mark_dead(&short, &local_id, true).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracked(short: &str) -> Liveness {
        let mut l = Liveness::default();
        l.set_host(Path::new("/tmp/control.sock"));
        l.observe(short, "sess", Some(1));
        l
    }

    #[test]
    fn one_miss_is_not_death_two_consecutive_are() {
        let mut l = tracked("w1");
        let t = Instant::now();
        assert_eq!(l.record("w1", Verdict::Gone, t), None);
        assert!(!l.is_confirmed_dead("w1"));
        assert_eq!(l.record("w1", Verdict::Gone, t + RECHECK_INTERVAL), Some("sess".into()));
        assert!(l.is_confirmed_dead("w1"));
        assert_eq!(l.record("w1", Verdict::Gone, t + RECHECK_INTERVAL * 2), None);
    }

    #[test]
    fn an_alive_answer_between_misses_resets_the_count() {
        let mut l = tracked("w1");
        let t = Instant::now();
        l.record("w1", Verdict::Gone, t);
        l.record("w1", Verdict::Alive, t);
        assert_eq!(l.record("w1", Verdict::Gone, t), None);
        assert!(!l.is_confirmed_dead("w1"));
    }

    #[test]
    fn unknown_never_counts_as_a_miss() {
        let mut l = tracked("w1");
        let t = Instant::now();
        for _ in 0..5 {
            assert_eq!(l.record("w1", Verdict::Unknown, t), None);
        }
        assert!(!l.is_confirmed_dead("w1"));
    }

    #[test]
    fn alive_or_a_respawn_clears_a_confirmed_death() {
        let mut l = tracked("w1");
        let t = Instant::now();
        l.record("w1", Verdict::Gone, t);
        l.record("w1", Verdict::Gone, t);
        l.record("w1", Verdict::Alive, t);
        assert!(!l.is_confirmed_dead("w1"));

        l.record("w1", Verdict::Gone, t);
        l.record("w1", Verdict::Gone, t);
        l.observe("w1", "sess", Some(2));
        assert!(!l.is_confirmed_dead("w1"));
    }

    #[test]
    fn probes_are_throttled_per_worker() {
        let mut l = tracked("w1");
        let t = Instant::now();
        assert_eq!(l.due(t), vec!["w1".to_owned()]);
        l.record("w1", Verdict::Alive, t);
        assert!(l.due(t + Duration::from_secs(5)).is_empty());
        assert_eq!(l.due(t + PROBE_INTERVAL), vec!["w1".to_owned()]);
        l.record("w1", Verdict::Gone, t + PROBE_INTERVAL);
        assert_eq!(l.due(t + PROBE_INTERVAL + RECHECK_INTERVAL), vec!["w1".to_owned()]);
    }

    #[test]
    fn a_different_socket_does_not_speak_for_the_roster() {
        let mut l = tracked("w1");
        assert!(!l.hosts(Path::new("/tmp/other.sock")));
        let t = Instant::now();
        l.record("w1", Verdict::Gone, t);
        l.set_host(Path::new("/tmp/other.sock"));
        assert_eq!(l.record("w1", Verdict::Gone, t), None);
    }

    #[tokio::test]
    async fn a_socket_error_is_unknown_not_dead() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(probe(&tmp.path().join("missing.sock"), "w1").await, Verdict::Unknown);
    }

    fn has_server(
        alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> (tempfile::TempDir, PathBuf) {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
        let tmp = tempfile::tempdir().unwrap();
        let sock = tmp.path().join("control.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                tokio::io::BufReader::new(read).read_line(&mut line).await.unwrap();
                let request: serde_json::Value = serde_json::from_str(&line).unwrap();
                assert_eq!(request["op"], "has");
                let reply =
                    json!({"ok":true,"alive":alive.load(std::sync::atomic::Ordering::SeqCst)});
                write.write_all(format!("{reply}\n").as_bytes()).await.unwrap();
            }
        });
        (tmp, sock)
    }

    fn rewind(d: &mut Driver, short: &str) {
        d.liveness.workers.get_mut(short).unwrap().last_probe = None;
    }

    #[tokio::test]
    async fn two_gone_probes_hibernate_a_listed_worker_and_alive_revives_it() {
        use super::super::test_support::{driver, snap};
        use cctui_proto::adapter::AdapterEvent;
        let alive = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (_tmp, sock) = has_server(alive.clone());
        let (mut d, mut rx) = driver();
        d.liveness.set_host(&sock);
        d.apply_snapshot(vec![snap("dddd0004", "working", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::SessionStarted { .. }));
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));

        d.probe_liveness(&sock).await;
        assert!(rx.try_recv().is_err(), "one miss is not death");
        rewind(&mut d, "dddd0004");
        d.probe_liveness(&sock).await;
        assert!(matches!(
            rx.recv().await.unwrap(),
            AdapterEvent::Status { tempo: Some(ref t), .. } if t == "hibernated"
        ));

        d.apply_snapshot(vec![snap("dddd0004", "working", None)]).await;
        assert!(rx.try_recv().is_err(), "a live-looking list entry must not re-green it");

        alive.store(true, std::sync::atomic::Ordering::SeqCst);
        rewind(&mut d, "dddd0004");
        d.probe_liveness(&sock).await;
        d.apply_snapshot(vec![snap("dddd0004", "working", None)]).await;
        assert!(matches!(rx.recv().await.unwrap(), AdapterEvent::Status { .. }));
    }

    #[tokio::test]
    async fn foreign_workers_are_never_probed() {
        use super::super::test_support::{driver, snap};
        let (mut d, _rx) = driver();
        d.liveness.set_host(Path::new("/tmp/control.sock"));
        let mut human = snap("eeee0005", "working", None);
        human.source = Some("human".into());
        d.apply_snapshot(vec![human]).await;
        assert!(d.liveness.due(Instant::now()).is_empty());
    }
}
