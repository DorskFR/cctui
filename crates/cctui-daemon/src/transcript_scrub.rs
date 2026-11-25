//! Rewrites keep every line's byte length.
//!
//! The tailers and the server's resume marks are byte offsets into these
//! files, so a scrubbed line is padded with trailing spaces and a line that
//! would grow is left as is.

mod audit;
mod liveness;
mod rewrite;
mod sweep;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use cctui_crypto::redact::CompiledPatterns;
use cctui_proto::api::privacy_scan::ScanSample;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

pub use self::audit::Finding;
pub use self::liveness::Liveness;
pub use self::rewrite::{FileOutcome, Mode, Stamp, process_file};
pub use self::sweep::{Ledger, compile, configure, scrub_machine};

pub const IDLE: Duration = Duration::from_mins(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Harness {
    ClaudeCode,
    Codex,
}

impl Harness {
    /// Codex rollouts are only audited: rewriting them is not proven safe.
    const fn rewritable(self) -> bool {
        matches!(self, Self::ClaudeCode)
    }
}

#[derive(Debug, Clone)]
pub struct Roots {
    pub claude_projects: PathBuf,
    pub claude_registry: PathBuf,
    pub codex_sessions: Option<PathBuf>,
}

impl Roots {
    #[must_use]
    pub fn of_machine() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let claude = std::env::var_os("CLAUDE_CONFIG_DIR")
            .map_or_else(|| home.join(".claude"), PathBuf::from);
        let codex =
            std::env::var_os("CODEX_HOME").map_or_else(|| home.join(".codex"), PathBuf::from);
        Self {
            claude_projects: claude.join("projects"),
            claude_registry: claude.join("sessions"),
            codex_sessions: Some(codex.join("sessions")),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    pub mode: Mode,
    /// Live files are then read, never rewritten.
    pub scan_live: bool,
    pub collect: bool,
    pub idle: Duration,
}

#[derive(Debug, Default, Serialize)]
pub struct Report {
    pub dry_run: bool,
    pub files_scanned: usize,
    pub files_changed: usize,
    pub files_read_only: usize,
    pub files_live: usize,
    pub files_busy: usize,
    pub files_failed: usize,
    pub lines_changed: usize,
    pub lines_unfit: usize,
    pub substitutions: usize,
    pub by_category: BTreeMap<String, usize>,
    pub samples: BTreeMap<String, Vec<ScanSample>>,
    pub findings: Vec<Finding>,
    pub findings_truncated: bool,
}

impl Report {
    fn absorb(&mut self, out: &FileOutcome, harness: Harness, mode: Mode) {
        for (cat, n) in &out.stats {
            *self.by_category.entry(cat.clone()).or_insert(0) += n;
            self.substitutions += n;
        }
        self.lines_unfit += out.lines_unfit;
        if out.busy {
            self.files_busy += 1;
        } else if out.lines_changed > 0 && !harness.rewritable() {
            self.files_read_only += 1;
        } else if out.lines_changed > 0 && (out.written || mode == Mode::DryRun) {
            self.files_changed += 1;
            self.lines_changed += out.lines_changed;
        }
    }
}

#[derive(Debug, Clone)]
pub struct Target {
    pub path: PathBuf,
    pub harness: Harness,
}

impl Target {
    /// The session a file belongs to: its stem, or the parent session of a
    /// subagent transcript (`<session>/subagents/<agent>.jsonl`).
    fn session(&self) -> String {
        let parent = self.path.parent();
        if parent.and_then(Path::file_name).is_some_and(|n| n == "subagents")
            && let Some(name) = parent.and_then(Path::parent).and_then(Path::file_name)
        {
            return name.to_string_lossy().into_owned();
        }
        self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }
}

#[must_use]
pub fn targets(roots: &Roots) -> Vec<Target> {
    let mut out = Vec::new();
    collect_jsonl(&roots.claude_projects, Harness::ClaudeCode, &mut out);
    if let Some(codex) = &roots.codex_sessions {
        collect_jsonl(codex, Harness::Codex, &mut out);
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn collect_jsonl(dir: &Path, harness: Harness, out: &mut Vec<Target>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let path = entry.path();
        if kind.is_dir() {
            collect_jsonl(&path, harness, out);
        } else if kind.is_file() && path.extension().is_some_and(|e| e == "jsonl") {
            out.push(Target { path, harness });
        }
    }
}

pub fn run(
    targets: &[Target],
    liveness: &Liveness,
    patterns: &CompiledPatterns,
    opts: Options,
    stop: &CancellationToken,
    mut ledger: Option<&mut Ledger>,
) -> Report {
    let mut report = Report { dry_run: opts.mode == Mode::DryRun, ..Report::default() };
    let mut audit = opts.collect.then(audit::Audit::default);
    for target in targets {
        if stop.is_cancelled() {
            break;
        }
        let stamp = Stamp::of(&target.path).ok();
        if let (Some(ledger), Some(stamp)) = (ledger.as_deref(), stamp)
            && ledger.settled(&target.path, stamp)
        {
            continue;
        }
        let live = stamp.is_none_or(|s| liveness.is_live(&target.path, &s, opts.idle));
        if live {
            report.files_live += 1;
            if !opts.scan_live {
                continue;
            }
        }
        let mode = if live || !target.harness.rewritable() { Mode::DryRun } else { opts.mode };
        report.files_scanned += 1;
        match process_file(target, patterns, mode, audit.as_mut()) {
            Ok(out) => {
                report.absorb(&out, target.harness, opts.mode);
                if let Some(ledger) = ledger.as_deref_mut()
                    && !out.busy
                    && !live
                {
                    ledger.settle(&target.path, out.stamp);
                }
            }
            Err(err) => {
                tracing::debug!(path = %target.path.display(), %err, "transcript scrub failed");
                report.files_failed += 1;
            }
        }
    }
    if let Some(audit) = audit {
        audit.into_report(&mut report);
    }
    report
}
