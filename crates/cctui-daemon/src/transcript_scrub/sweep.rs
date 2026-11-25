use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use cctui_crypto::redact::{self, CompiledPatterns};
use cctui_proto::api::privacy_scan::TranscriptAuditReport;
use cctui_proto::ws::{DaemonFrameUp, SecretScrubConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::{IDLE, Liveness, Mode, Options, Report, Roots, Stamp, run, targets};

const FIRST_SWEEP_DELAY: Duration = Duration::from_mins(5);
const SWEEP_EVERY: Duration = Duration::from_mins(15);

struct Running {
    key: String,
    stop: CancellationToken,
}

static SWEEP: Mutex<Option<Running>> = Mutex::new(None);

static LAST_CONFIG: Mutex<Option<SecretScrubConfig>> = Mutex::new(None);

/// One pass over the transcripts at a time, so a requested audit never
/// rewrites a file the background sweep is rewriting.
static PASS: Mutex<()> = Mutex::new(());

pub fn configure(cfg: &SecretScrubConfig) {
    *LAST_CONFIG.lock().unwrap_or_else(PoisonError::into_inner) = Some(cfg.clone());
    let mut slot = SWEEP.lock().unwrap_or_else(PoisonError::into_inner);
    let key = sweeps(cfg).then(|| patterns_key(cfg));
    if slot.as_ref().map(|r| &r.key) == key.as_ref() {
        return;
    }
    if let Some(running) = slot.take() {
        running.stop.cancel();
    }
    let Some(key) = key else { return };
    let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
    let stop = CancellationToken::new();
    runtime.spawn(sweep_loop(cfg.clone(), key.clone(), stop.clone()));
    *slot = Some(Running { key, stop });
}

/// The local sweep follows the main secret scrub: it never runs without it.
pub(super) const fn sweeps(cfg: &SecretScrubConfig) -> bool {
    cfg.enabled && cfg.local_transcripts
}

async fn sweep_loop(cfg: SecretScrubConfig, key: String, stop: CancellationToken) {
    let mut wait = FIRST_SWEEP_DELAY;
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            () = tokio::time::sleep(wait) => {}
        }
        wait = SWEEP_EVERY;
        let (cfg, key, token) = (cfg.clone(), key.clone(), stop.clone());
        match tokio::task::spawn_blocking(move || sweep_once(&cfg, &key, &token)).await {
            Ok(r) if r.files_changed + r.files_busy + r.files_failed > 0 => tracing::info!(
                files_changed = r.files_changed,
                lines_changed = r.lines_changed,
                lines_unfit = r.lines_unfit,
                files_busy = r.files_busy,
                files_failed = r.files_failed,
                "scrubbed local transcripts"
            ),
            Ok(_) => {}
            Err(err) => tracing::warn!(%err, "local transcript sweep panicked"),
        }
    }
}

fn sweep_once(cfg: &SecretScrubConfig, key: &str, stop: &CancellationToken) -> Report {
    let _pass = PASS.lock().unwrap_or_else(PoisonError::into_inner);
    let roots = Roots { codex_sessions: None, ..Roots::of_machine() };
    let tmp_removed = roots.clear_stale_tmp();
    let patterns = compile(cfg);
    let all = targets(&roots);
    let mut ledger = Ledger::open(ledger_path(), key);
    let opts = Options { mode: Mode::Apply, scan_live: false, collect: false, idle: IDLE };
    let mut report = run(&all, &Liveness::probe(&roots), &patterns, opts, stop, Some(&mut ledger));
    report.tmp_removed = tmp_removed;
    let present: HashSet<String> = all.iter().map(|t| ledger_key(&t.path)).collect();
    ledger.files.retain(|k, _| present.contains(k));
    ledger.flush();
    report
}

/// Built-ins plus the user patterns of the last `Reconcile`; live files and
/// codex rollouts are read, never rewritten. An apply runs as a dry run while
/// the local sweep is off.
#[must_use]
pub fn audit_for_server(dry_run: bool) -> TranscriptAuditReport {
    let user = LAST_CONFIG.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let user = user.unwrap_or_default();
    let mode = audit_mode(dry_run, &user);
    let cfg = SecretScrubConfig { enabled: true, ..user };
    let roots = Roots::of_machine();
    let opts = Options { mode, scan_live: true, collect: true, idle: IDLE };
    let _pass = PASS.lock().unwrap_or_else(PoisonError::into_inner);
    if mode == Mode::Apply {
        let _ = roots.clear_stale_tmp();
    }
    let all = targets(&roots);
    let stop = CancellationToken::new();
    run(&all, &Liveness::probe(&roots), &compile(&cfg), opts, &stop, None).to_audit()
}

pub(super) const fn audit_mode(dry_run: bool, last: &SecretScrubConfig) -> Mode {
    if dry_run || !sweeps(last) { Mode::DryRun } else { Mode::Apply }
}

pub(super) const AUDIT_BUSY: &str = "another transcript audit is running on this machine";

type Waiter = (uuid::Uuid, mpsc::Sender<DaemonFrameUp>);

struct InFlight {
    dry_run: bool,
    waiters: Vec<Waiter>,
}

/// One audit pass per daemon: a request for the same mode joins the running
/// pass and gets its result, any other is refused.
pub(super) struct AuditGate(Mutex<Option<InFlight>>);

#[derive(Debug, PartialEq, Eq)]
enum Admission {
    Start,
    Joined,
    Busy,
}

impl AuditGate {
    #[must_use]
    pub(super) const fn new() -> Self {
        Self(Mutex::new(None))
    }

    fn admit(&self, waiter: Waiter, dry_run: bool) -> Admission {
        Self::admit_into(
            &mut self.0.lock().unwrap_or_else(PoisonError::into_inner),
            waiter,
            dry_run,
        )
    }

    fn admit_into(slot: &mut Option<InFlight>, waiter: Waiter, dry_run: bool) -> Admission {
        match slot.as_mut() {
            None => {
                *slot = Some(InFlight { dry_run, waiters: vec![waiter] });
                Admission::Start
            }
            Some(running) if running.dry_run == dry_run => {
                running.waiters.push(waiter);
                Admission::Joined
            }
            Some(_) => Admission::Busy,
        }
    }

    fn finish(&self) -> Vec<Waiter> {
        let running = self.0.lock().unwrap_or_else(PoisonError::into_inner).take();
        running.map(|running| running.waiters).unwrap_or_default()
    }
}

static AUDIT: AuditGate = AuditGate::new();

pub fn serve_audit(request_id: uuid::Uuid, dry_run: bool, up: mpsc::Sender<DaemonFrameUp>) {
    serve_through(&AUDIT, request_id, dry_run, up, audit_for_server);
}

pub(super) fn serve_through<F>(
    gate: &'static AuditGate,
    request_id: uuid::Uuid,
    dry_run: bool,
    up: mpsc::Sender<DaemonFrameUp>,
    audit: F,
) where
    F: FnOnce(bool) -> TranscriptAuditReport + Send + 'static,
{
    match gate.admit((request_id, up.clone()), dry_run) {
        Admission::Start => {}
        Admission::Joined => return,
        Admission::Busy => {
            tokio::spawn(reply(up, request_id, Err(AUDIT_BUSY.to_owned())));
            return;
        }
    }
    tokio::spawn(async move {
        let outcome = tokio::task::spawn_blocking(move || audit(dry_run))
            .await
            .map_err(|err| format!("transcript audit stopped: {err}"));
        for (request_id, up) in gate.finish() {
            reply(up, request_id, outcome.clone()).await;
        }
    });
}

async fn reply(
    up: mpsc::Sender<DaemonFrameUp>,
    request_id: uuid::Uuid,
    outcome: Result<TranscriptAuditReport, String>,
) {
    let (report, error) = match outcome {
        Ok(report) => (Some(report), None),
        Err(err) => (None, Some(err)),
    };
    let frame = DaemonFrameUp::TranscriptAuditResult { request_id, report, error };
    if up.send(frame).await.is_err() {
        tracing::warn!("frame_up channel closed; dropping TranscriptAuditResult");
    }
}

/// Built-in detectors only: the user's patterns live on the server.
#[must_use]
pub fn scrub_machine(mode: Mode) -> Report {
    let cfg = SecretScrubConfig { enabled: true, ..SecretScrubConfig::default() };
    let roots = Roots::of_machine();
    let tmp_removed = if mode == Mode::Apply { roots.clear_stale_tmp() } else { 0 };
    let all = targets(&roots);
    let opts = Options { mode, scan_live: true, collect: true, idle: IDLE };
    let stop = CancellationToken::new();
    let mut report = run(&all, &Liveness::probe(&roots), &compile(&cfg), opts, &stop, None);
    report.tmp_removed = tmp_removed;
    report
}

#[must_use]
pub fn compile(cfg: &SecretScrubConfig) -> CompiledPatterns {
    let user: Vec<(String, String)> =
        cfg.patterns.iter().map(|p| (p.name.clone(), p.regex.clone())).collect();
    redact::compile(cfg.enabled, &user, &cctui_crypto::vault_key())
}

/// Built-in detectors change between daemon versions, so the version is part
/// of the key and an upgrade rescans every file.
fn patterns_key(cfg: &SecretScrubConfig) -> String {
    let builtins: Vec<&str> = redact::builtin_categories().into_iter().map(|(c, _)| c).collect();
    detector_key(env!("CARGO_PKG_VERSION"), &builtins, cfg)
}

#[must_use]
pub(super) fn detector_key(version: &str, builtins: &[&str], cfg: &SecretScrubConfig) -> String {
    let mut hash = Sha256::new();
    hash.update([u8::from(cfg.enabled)]);
    for part in std::iter::once(version).chain(builtins.iter().copied()) {
        hash.update(part.len().to_le_bytes());
        hash.update(part.as_bytes());
    }
    for p in &cfg.patterns {
        for part in [&p.name, &p.regex] {
            hash.update(part.len().to_le_bytes());
            hash.update(part.as_bytes());
        }
    }
    hex::encode(hash.finalize())
}

fn ledger_path() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("cctui").join("transcript-scrub-ledger.json"))
}

fn ledger_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

type StampKey = (u64, u128, u64);

fn stamp_key(stamp: Stamp) -> Option<StampKey> {
    let modified = stamp.modified?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some((stamp.len, modified, stamp.inode))
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Ledger {
    patterns: String,
    files: HashMap<String, StampKey>,
    #[serde(skip)]
    path: Option<PathBuf>,
}

impl Ledger {
    #[must_use]
    pub fn open(path: Option<PathBuf>, patterns: &str) -> Self {
        let stored: Option<Self> = path
            .as_ref()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|raw| serde_json::from_slice(&raw).ok());
        let files = stored.filter(|l| l.patterns == patterns).map(|l| l.files).unwrap_or_default();
        Self { patterns: patterns.to_owned(), files, path }
    }

    #[must_use]
    pub fn settled(&self, path: &Path, stamp: Stamp) -> bool {
        stamp_key(stamp).is_some_and(|k| self.files.get(&ledger_key(path)) == Some(&k))
    }

    pub fn settle(&mut self, path: &Path, stamp: Stamp) {
        if let Some(k) = stamp_key(stamp) {
            self.files.insert(ledger_key(path), k);
        }
    }

    pub fn flush(&self) {
        let Some(path) = &self.path else { return };
        let Ok(bytes) = serde_json::to_vec(self) else { return };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
        if let Err(err) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
            let _ = std::fs::remove_file(&tmp);
            tracing::debug!(%err, "transcript scrub ledger not saved");
        }
    }
}
