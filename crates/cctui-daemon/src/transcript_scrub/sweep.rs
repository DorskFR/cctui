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
    let key = cfg.local_transcripts.then(|| patterns_key(cfg));
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
    let patterns = compile(cfg);
    let all = targets(&roots);
    let mut ledger = Ledger::open(ledger_path(), key);
    let opts = Options { mode: Mode::Apply, scan_live: false, collect: false, idle: IDLE };
    let report = run(&all, &Liveness::probe(&roots), &patterns, opts, stop, Some(&mut ledger));
    let present: HashSet<String> = all.iter().map(|t| ledger_key(&t.path)).collect();
    ledger.files.retain(|k, _| present.contains(k));
    ledger.flush();
    report
}

/// Built-ins plus the user patterns of the last `Reconcile`; live files and
/// codex rollouts are read, never rewritten.
#[must_use]
pub fn audit_for_server(dry_run: bool) -> TranscriptAuditReport {
    let user = LAST_CONFIG.lock().unwrap_or_else(PoisonError::into_inner).clone();
    let cfg = SecretScrubConfig { enabled: true, ..user.unwrap_or_default() };
    let roots = Roots::of_machine();
    let mode = if dry_run { Mode::DryRun } else { Mode::Apply };
    let opts = Options { mode, scan_live: true, collect: true, idle: IDLE };
    let _pass = PASS.lock().unwrap_or_else(PoisonError::into_inner);
    let all = targets(&roots);
    let stop = CancellationToken::new();
    run(&all, &Liveness::probe(&roots), &compile(&cfg), opts, &stop, None).to_audit()
}

pub fn serve_audit(request_id: uuid::Uuid, dry_run: bool, up: mpsc::Sender<DaemonFrameUp>) {
    tokio::spawn(async move {
        let reply = match tokio::task::spawn_blocking(move || audit_for_server(dry_run)).await {
            Ok(report) => DaemonFrameUp::TranscriptAuditResult {
                request_id,
                report: Some(report),
                error: None,
            },
            Err(err) => DaemonFrameUp::TranscriptAuditResult {
                request_id,
                report: None,
                error: Some(format!("transcript audit stopped: {err}")),
            },
        };
        if up.send(reply).await.is_err() {
            tracing::warn!("frame_up channel closed; dropping TranscriptAuditResult");
        }
    });
}

/// Built-in detectors only: the user's patterns live on the server.
#[must_use]
pub fn scrub_machine(mode: Mode) -> Report {
    let cfg = SecretScrubConfig { enabled: true, ..SecretScrubConfig::default() };
    let roots = Roots::of_machine();
    let all = targets(&roots);
    let opts = Options { mode, scan_live: true, collect: true, idle: IDLE };
    let stop = CancellationToken::new();
    run(&all, &Liveness::probe(&roots), &compile(&cfg), opts, &stop, None)
}

#[must_use]
pub fn compile(cfg: &SecretScrubConfig) -> CompiledPatterns {
    let user: Vec<(String, String)> =
        cfg.patterns.iter().map(|p| (p.name.clone(), p.regex.clone())).collect();
    redact::compile(cfg.enabled, &user, &cctui_crypto::vault_key())
}

fn patterns_key(cfg: &SecretScrubConfig) -> String {
    let mut hash = Sha256::new();
    hash.update([u8::from(cfg.enabled)]);
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
