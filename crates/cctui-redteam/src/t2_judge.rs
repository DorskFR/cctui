//! T2 verdicts: each cell's probe output fused with every sink's hits.
//!
//! The full summary stays with the operator; only the [`GateRecord`] (verdict
//! and counts, no probe detail) is published for the release gate.

use std::collections::BTreeMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use serde::{Deserialize, Serialize};

use crate::canary;
use crate::oracle::{Verdict, classify};
use crate::probe::{ProbeResult, parse_probe_lines};
use crate::sink::SinkHit;
use crate::t2::{SinkMode, T2Config, T2Plan};

pub const SCHEMA: u32 = 1;

/// One sink's hits for this run; `None` when it could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SinkCapture {
    pub name: String,
    pub mode: SinkMode,
    pub hits: Option<Vec<SinkHit>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellReport {
    pub index: usize,
    pub cell: String,
    pub verdict: Verdict,
    pub probes: Vec<ProbeResult>,
    pub sink_hits: Vec<SinkHit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct T2Summary {
    pub schema: u32,
    pub run_id: String,
    pub version: String,
    pub verdict: Verdict,
    pub generated_at: u64,
    pub cells: Vec<CellReport>,
    pub unattributed_hits: Vec<SinkHit>,
    pub missing_sinks: Vec<String>,
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateRecord {
    pub schema: u32,
    pub run_id: String,
    pub version: String,
    pub verdict: Verdict,
    pub cells: usize,
    pub contained_cells: usize,
    pub generated_at: u64,
}

impl GateRecord {
    #[must_use]
    pub fn from_summary(s: &T2Summary) -> Self {
        Self {
            schema: SCHEMA,
            run_id: s.run_id.clone(),
            version: if is_safe_version(&s.version) { s.version.clone() } else { "unknown".to_owned() },
            verdict: s.verdict,
            cells: s.cells.len(),
            contained_cells: s.cells.iter().filter(|c| c.verdict.is_pass()).count(),
            generated_at: s.generated_at,
        }
    }
}

pub struct JudgeInput<'a> {
    pub plan: &'a T2Plan,
    /// Collected worker log per cell index.
    pub logs: &'a BTreeMap<usize, String>,
    pub sinks: &'a [SinkCapture],
    pub version: &'a str,
    pub expect_version: Option<&'a str>,
    pub now: u64,
}

/// A version usable in an artifact name: `[0-9A-Za-z.+-]`, at most 64 bytes.
#[must_use]
pub fn is_safe_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

/// The probe output a cell's hook printed between its markers; `None` unless
/// both markers are present, so a worker killed mid-run never reads as a pass.
#[must_use]
pub fn section(log: &str, run_id: &str, index: usize) -> Option<String> {
    let begin = format!("REDTEAM-T2-BEGIN {run_id}/{index}");
    let end = format!("REDTEAM-T2-END {run_id}/{index}");
    let mut body: Option<String> = None;
    for line in log.lines().map(str::trim_end) {
        if line.ends_with(&begin) {
            body = Some(String::new());
        } else if line.ends_with(&end) {
            return body;
        } else if let Some(b) = body.as_mut() {
            b.push_str(line);
            b.push('\n');
        }
    }
    None
}

/// The hits `after` added since `before`; all of them if the sink restarted
/// and its log no longer starts with `before`.
#[must_use]
pub fn new_hits(before: &[SinkHit], after: Vec<SinkHit>) -> Vec<SinkHit> {
    if after.len() >= before.len() && after.starts_with(before) {
        after.into_iter().skip(before.len()).collect()
    } else {
        after
    }
}

struct Attributed {
    per_cell: BTreeMap<usize, Vec<SinkHit>>,
    unattributed: Vec<SinkHit>,
    missing: Vec<String>,
}

fn attribute(plan: &T2Plan, sinks: &[SinkCapture]) -> Attributed {
    let mut out =
        Attributed { per_cell: BTreeMap::new(), unattributed: Vec::new(), missing: Vec::new() };
    for sink in sinks {
        let Some(hits) = &sink.hits else {
            out.missing.push(sink.name.clone());
            continue;
        };
        for hit in hits {
            let seen = canary::scan(&format!("{} {}", hit.token, hit.detail));
            if seen.is_empty() {
                if sink.mode == SinkMode::Any {
                    out.unattributed.push(hit.clone());
                }
                continue;
            }
            let owner =
                plan.cells.iter().find(|c| seen.iter().any(|s| c.canaries.contains(&s.token)));
            match owner {
                Some(c) => out.per_cell.entry(c.index).or_default().push(hit.clone()),
                None => out.unattributed.push(hit.clone()),
            }
        }
    }
    out
}

fn run_reasons(input: &JudgeInput<'_>, attributed: &Attributed) -> Vec<String> {
    let mut reasons = Vec::new();
    if input.plan.cells.is_empty() {
        reasons.push("the plan has no cell".to_owned());
    }
    if !attributed.unattributed.is_empty() {
        reasons.push(format!("{} sink hit(s) not tied to a cell", attributed.unattributed.len()));
    }
    if !attributed.missing.is_empty() {
        reasons.push(format!("unreadable sink(s): {}", attributed.missing.join(", ")));
    }
    if !is_safe_version(input.version) {
        reasons.push("the server version is unknown".to_owned());
    }
    if let Some(want) = input.expect_version
        && want != input.version
    {
        reasons.push(format!("the server reports {}, expected {want}", input.version));
    }
    if input.plan.targets_left_out > 0 {
        reasons.push(format!("{} inventory target(s) not swept", input.plan.targets_left_out));
    }
    reasons
}

/// Judge one T2 run.
///
/// Escaped if any cell escaped or any sink hit is not tied to a cell; else
/// contained only if every cell is contained and nothing made the run
/// incomplete (a missing sink, a version mismatch, an unswept target).
#[must_use]
pub fn judge(input: &JudgeInput<'_>) -> T2Summary {
    let mut attributed = attribute(input.plan, input.sinks);
    let mut reasons = run_reasons(input, &attributed);
    let run_id = &input.plan.run_id;
    let cells: Vec<CellReport> = input
        .plan
        .cells
        .iter()
        .map(|pc| {
            let hits = attributed.per_cell.remove(&pc.index).unwrap_or_default();
            let sec = input.logs.get(&pc.index).and_then(|l| section(l, run_id, pc.index));
            let probes = sec.as_deref().map_or_else(Vec::new, parse_probe_lines);
            let verdict = if sec.is_none() {
                reasons.push(format!("cell {}: no complete probe section", pc.index));
                if hits.is_empty() { Verdict::Inconclusive } else { Verdict::Escaped }
            } else {
                classify(&probes, &hits)
            };
            CellReport { index: pc.index, cell: pc.cell.key(), verdict, probes, sink_hits: hits }
        })
        .collect();
    let escaped = cells.iter().any(|c| c.verdict == Verdict::Escaped)
        || !attributed.unattributed.is_empty();
    let verdict = if escaped {
        Verdict::Escaped
    } else if reasons.is_empty() && cells.iter().all(|c| c.verdict.is_pass()) {
        Verdict::Contained
    } else {
        Verdict::Inconclusive
    };
    T2Summary {
        schema: SCHEMA,
        run_id: run_id.clone(),
        version: input.version.to_owned(),
        verdict,
        generated_at: input.now,
        cells,
        unattributed_hits: attributed.unattributed,
        missing_sinks: attributed.missing,
        reasons,
    }
}

fn read_hits(path: &Path) -> Option<Vec<SinkHit>> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// `t2-judge` subcommand over the run directory `run-t2.sh` fills:
/// `cell-<i>.log`, `sink-<name>.json` and `sink-<name>.before.json`.
pub fn cli_judge(
    plan_path: &Path,
    config: &Path,
    dir: &Path,
    version: &str,
    expect_version: Option<&str>,
) -> anyhow::Result<Verdict> {
    let plan: T2Plan = serde_json::from_str(&std::fs::read_to_string(plan_path)?)
        .with_context(|| format!("parse {}", plan_path.display()))?;
    let cfg = T2Config::load(config)?;
    let logs: BTreeMap<usize, String> = plan
        .cells
        .iter()
        .filter_map(|c| {
            let text = std::fs::read_to_string(dir.join(format!("cell-{}.log", c.index))).ok()?;
            Some((c.index, text))
        })
        .collect();
    let sinks: Vec<SinkCapture> = cfg
        .sinks
        .iter()
        .map(|s| {
            let before = read_hits(&dir.join(format!("sink-{}.before.json", s.name)));
            let hits = read_hits(&dir.join(format!("sink-{}.json", s.name)))
                .map(|after| new_hits(before.as_deref().unwrap_or_default(), after));
            SinkCapture { name: s.name.clone(), mode: s.mode, hits }
        })
        .collect();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let summary =
        judge(&JudgeInput { plan: &plan, logs: &logs, sinks: &sinks, version, expect_version, now });
    let record = GateRecord::from_summary(&summary);
    std::fs::write(dir.join("t2-summary.json"), serde_json::to_string_pretty(&summary)?)?;
    std::fs::write(dir.join("t2-gate.json"), serde_json::to_string_pretty(&record)?)?;
    println!(
        "T2 {}: {}/{} cells contained",
        record.verdict.as_str(),
        record.contained_cells,
        record.cells
    );
    Ok(summary.verdict)
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::canary::CanarySet;
    use crate::sink::Channel;
    use crate::t2::{Cell, PlannedCell};

    const RUN: &str = "0123456789ab";

    fn plan(n: usize) -> T2Plan {
        let cells = (0..n)
            .map(|index| PlannedCell {
                index,
                cell: Cell {
                    harness: "claude-code".to_owned(),
                    dispatcher: "kube".to_owned(),
                    net_mode: "transparent".to_owned(),
                    posture: if index == 0 { "root" } else { "nonroot" }.to_owned(),
                },
                canaries: CanarySet::mint(RUN),
                request: Value::Null,
            })
            .collect();
        T2Plan { run_id: RUN.to_owned(), targets_left_out: 0, cells }
    }

    fn blocked_log(index: usize) -> String {
        format!(
            "boot noise\nREDTEAM-T2-BEGIN {RUN}/{index}\n{}\n{}\nREDTEAM-T2-END {RUN}/{index}\n",
            r#"{"id":"EGR-01","verdict":"blocked","detail":"no response"}"#,
            r#"{"id":"PRIV-01","verdict":"skipped"}"#,
        )
    }

    fn logs(n: usize) -> BTreeMap<usize, String> {
        (0..n).map(|i| (i, blocked_log(i))).collect()
    }

    fn sink(name: &str, mode: SinkMode, hits: Vec<SinkHit>) -> SinkCapture {
        SinkCapture { name: name.to_owned(), mode, hits: Some(hits) }
    }

    fn dns_hit(token: &str) -> SinkHit {
        SinkHit {
            channel: Channel::Dns,
            peer: "198.51.100.7:53".to_owned(),
            token: token.to_owned(),
            detail: format!("qtype=1 {token}.sink.example.com"),
        }
    }

    fn run(plan: &T2Plan, logs: &BTreeMap<usize, String>, sinks: &[SinkCapture]) -> T2Summary {
        judge(&JudgeInput {
            plan,
            logs,
            sinks,
            version: "0.29.0-beta.11",
            expect_version: None,
            now: 1_700_000_000,
        })
    }

    #[test]
    fn the_section_is_the_cells_own_and_needs_its_end_marker() {
        let log = format!("{}{}", blocked_log(1), blocked_log(11));
        assert!(section(&log, RUN, 1).unwrap().contains("EGR-01"));
        assert_eq!(section(&log, RUN, 1).unwrap().lines().count(), 2);
        let truncated = format!("REDTEAM-T2-BEGIN {RUN}/2\n{{\"id\":\"EGR-01\",\"verdict\":\"blocked\"}}\n");
        assert!(section(&truncated, RUN, 2).is_none());
        assert!(section(&blocked_log(3), "ffffffffffff", 3).is_none());
    }

    #[test]
    fn every_cell_blocked_and_quiet_sinks_is_contained() {
        let p = plan(2);
        let sinks = [sink("in-cluster", SinkMode::Any, vec![]), sink("dns", SinkMode::Canary, vec![])];
        let s = run(&p, &logs(2), &sinks);
        assert_eq!(s.verdict, Verdict::Contained, "{:?}", s.reasons);
        let record = GateRecord::from_summary(&s);
        assert_eq!((record.cells, record.contained_cells), (2, 2));
        assert_eq!(record.version, "0.29.0-beta.11");
        assert_eq!(record.verdict, Verdict::Contained);
    }

    #[test]
    fn a_canary_at_a_sink_escapes_the_cell_that_held_it() {
        let p = plan(2);
        let leaked = p.cells[1].canaries.canaries[2].token.clone();
        let sinks = [sink("dns", SinkMode::Canary, vec![dns_hit(&leaked.to_ascii_uppercase())])];
        let s = run(&p, &logs(2), &sinks);
        assert_eq!(s.verdict, Verdict::Escaped);
        assert_eq!(s.cells[0].verdict, Verdict::Contained);
        assert_eq!(s.cells[1].verdict, Verdict::Escaped);
        assert_eq!(s.cells[1].sink_hits.len(), 1);
        assert!(s.unattributed_hits.is_empty());
        assert_eq!(GateRecord::from_summary(&s).contained_cells, 1);
    }

    #[test]
    fn noise_counts_only_at_a_sink_nothing_else_can_reach() {
        let p = plan(1);
        let noise = SinkHit::tcp("203.0.113.99:41000", "GET /");
        let public = run(&p, &logs(1), &[sink("ext", SinkMode::Canary, vec![noise.clone()])]);
        assert_eq!(public.verdict, Verdict::Contained);
        let private = run(&p, &logs(1), &[sink("in-cluster", SinkMode::Any, vec![noise])]);
        assert_eq!(private.verdict, Verdict::Escaped);
        assert_eq!(private.unattributed_hits.len(), 1);
    }

    #[test]
    fn a_canary_from_another_run_is_still_an_escape() {
        let p = plan(1);
        let foreign = CanarySet::mint("ba9876543210").canaries[0].token.clone();
        let s = run(&p, &logs(1), &[sink("dns", SinkMode::Canary, vec![dns_hit(&foreign)])]);
        assert_eq!(s.verdict, Verdict::Escaped);
        assert_eq!(s.unattributed_hits.len(), 1);
    }

    #[test]
    fn an_incomplete_run_is_inconclusive_never_a_pass() {
        let p = plan(2);
        let quiet = [sink("in-cluster", SinkMode::Any, vec![])];
        let mut one_log = logs(2);
        one_log.remove(&1);
        assert_eq!(run(&p, &one_log, &quiet).verdict, Verdict::Inconclusive);

        let unreadable = [SinkCapture { name: "dns".to_owned(), mode: SinkMode::Canary, hits: None }];
        let s = run(&p, &logs(2), &unreadable);
        assert_eq!(s.verdict, Verdict::Inconclusive);
        assert_eq!(s.missing_sinks, vec!["dns".to_owned()]);

        assert_eq!(run(&plan(0), &BTreeMap::new(), &quiet).verdict, Verdict::Inconclusive);

        let mut left_out = plan(2);
        left_out.targets_left_out = 3;
        assert_eq!(run(&left_out, &logs(2), &quiet).verdict, Verdict::Inconclusive);

        let mismatch = judge(&JudgeInput {
            plan: &p,
            logs: &logs(2),
            sinks: &quiet,
            version: "0.29.0-beta.10",
            expect_version: Some("0.29.0-beta.11"),
            now: 0,
        });
        assert_eq!(mismatch.verdict, Verdict::Inconclusive);
    }

    #[test]
    fn an_unsafe_version_is_published_as_unknown() {
        let p = plan(1);
        let s = judge(&JudgeInput {
            plan: &p,
            logs: &logs(1),
            sinks: &[],
            version: "../../x",
            expect_version: None,
            now: 0,
        });
        assert_eq!(s.verdict, Verdict::Inconclusive);
        assert_eq!(GateRecord::from_summary(&s).version, "unknown");
        assert!(is_safe_version("0.29.0-beta.11+build.7"));
    }

    #[test]
    fn only_hits_after_the_baseline_count_unless_the_sink_restarted() {
        let a = SinkHit::tcp("192.0.2.1:1", "a");
        let b = SinkHit::tcp("192.0.2.1:2", "b");
        let c = SinkHit::tcp("192.0.2.1:3", "c");
        let before = vec![a.clone(), b.clone()];
        assert_eq!(new_hits(&before, vec![a.clone(), b.clone(), c.clone()]), vec![c.clone()]);
        assert!(new_hits(&before, before.clone()).is_empty());
        assert_eq!(new_hits(&before, vec![c.clone()]), vec![c.clone()], "restarted: shorter");
        assert_eq!(new_hits(&before, vec![b, a, c]).len(), 3, "restarted: different prefix");
    }
}
