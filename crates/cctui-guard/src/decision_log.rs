//! Append-only JSONL decision log and the end-of-run report aggregated from it.
//!
//! Every guard `/check` and `/transition`, and every guard-proxy egress verdict,
//! appends one self-describing JSON line — the source of truth for why a run did
//! what it did. Writes are best-effort and use `O_APPEND` (atomic per line on
//! POSIX), so the guard and the proxy can log to the same file from separate
//! processes without a lock. [`build_report`] folds the log back into a per-step
//! summary of denied tools, denied hosts, the transition timeline, and time per
//! step — the feedback loop that turns a silent wall into policy-tuning signal.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Which daemon emitted a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Guard,
    Proxy,
}

/// The event a record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// A `PreToolUse` tool-call decision.
    Check,
    /// A `/transition` attempt outcome (gate/judge included in `rule`).
    Transition,
    /// A step became active — the timeline anchor a stepless network verdict is
    /// attributed to.
    Enter,
    /// A guard-proxy egress verdict on a `host:port`.
    Network,
}

/// One appended decision line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Decision {
    /// RFC 3339 / ISO 8601 UTC millisecond timestamp.
    pub ts: String,
    pub source: Source,
    pub kind: Kind,
    /// The active (or subject) step; `Some` for guard records, `None` for a
    /// proxy record that does not know the workflow step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// The normalized subject: a tool-call string, a transition target, or a
    /// `host:port`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target: String,
    /// `allow` / `deny` (checks, network), or a transition state
    /// (`enter`/`allow`/`deny`).
    pub verdict: String,
    /// The matched rule or the denial reason; empty on a clean allow.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub rule: String,
}

/// Milliseconds-precision UTC timestamp without pulling in `chrono` (the guard
/// crate has no such dependency).
#[allow(clippy::many_single_char_names, clippy::similar_names)]
fn now_ts() -> String {
    let now =
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    let secs = now.as_secs();
    let millis = now.subsec_millis();
    let days = i64::try_from(secs / 86_400).unwrap_or(0);
    let tod = secs % 86_400;
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// Days since the Unix epoch → `(year, month, day)` (Howard Hinnant's algorithm).
#[allow(clippy::many_single_char_names, clippy::similar_names, clippy::cast_sign_loss)]
const fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A JSONL decision-log sink. An unset path makes every record a no-op, so the
/// feature is off unless the entrypoint passes `--decision-log`.
#[derive(Debug, Clone, Default)]
pub struct DecisionLog {
    path: Option<PathBuf>,
}

impl DecisionLog {
    #[must_use]
    pub fn new(path: Option<PathBuf>) -> Self {
        if let Some(parent) = path.as_ref().and_then(|p| p.parent()) {
            let _ = std::fs::create_dir_all(parent);
        }
        Self { path }
    }

    /// Whether a sink is configured.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.path.is_some()
    }

    /// The log path, if configured.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Append one record. Best-effort: any IO error is swallowed so logging never
    /// changes an allow/deny outcome.
    pub fn record(&self, decision: &Decision) {
        let Some(path) = &self.path else {
            return;
        };
        let Ok(mut line) = serde_json::to_string(decision) else {
            return;
        };
        line.push('\n');
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = f.write_all(line.as_bytes());
        }
    }

    /// Record a tool-call `/check` verdict.
    pub fn check(&self, step: i64, tool: &str, target: &str, allowed: bool, rule: &str) {
        self.record(&Decision {
            ts: now_ts(),
            source: Source::Guard,
            kind: Kind::Check,
            step: Some(step),
            tool: Some(tool.to_string()),
            target: target.to_string(),
            verdict: if allowed { "allow" } else { "deny" }.to_string(),
            rule: rule.to_string(),
        });
    }

    /// Record that `step` became the active step (a timeline anchor).
    pub fn enter(&self, step: i64) {
        self.record(&Decision {
            ts: now_ts(),
            source: Source::Guard,
            kind: Kind::Enter,
            step: Some(step),
            tool: None,
            target: String::new(),
            verdict: "enter".to_string(),
            rule: String::new(),
        });
    }

    /// Record a `/transition` attempt outcome. `verdict` is `allow`/`deny`;
    /// `rule` carries the gate/judge detail on a refusal.
    pub fn transition(&self, from: i64, target: &str, verdict: &str, rule: &str) {
        self.record(&Decision {
            ts: now_ts(),
            source: Source::Guard,
            kind: Kind::Transition,
            step: Some(from),
            tool: None,
            target: target.to_string(),
            verdict: verdict.to_string(),
            rule: rule.to_string(),
        });
    }

    /// Record a guard-proxy egress verdict on `host:port`.
    pub fn network(&self, host_port: &str, allowed: bool, rule: &str) {
        self.record(&Decision {
            ts: now_ts(),
            source: Source::Proxy,
            kind: Kind::Network,
            step: None,
            tool: None,
            target: host_port.to_string(),
            verdict: if allowed { "allow" } else { "deny" }.to_string(),
            rule: rule.to_string(),
        });
    }
}

/// Parse a decision log into records, skipping blank or malformed lines.
#[must_use]
pub fn parse_log(text: &str) -> Vec<Decision> {
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<Decision>(l).ok())
        .collect()
}

/// Read a decision log file and aggregate it into the end-of-run report. A
/// missing file yields an empty report.
#[must_use]
pub fn build_report(path: &Path) -> Value {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    report_from_records(&parse_log(&text))
}

/// The step active at `ts`, from the `Enter` timeline: the last step entered at
/// or before `ts`. `None` before the first enter record.
fn step_at(timeline: &[(String, i64)], ts: &str) -> Option<i64> {
    timeline.iter().take_while(|(t, _)| t.as_str() <= ts).last().map(|(_, s)| *s)
}

#[derive(Default)]
struct Agg {
    count: u64,
    first: String,
    last: String,
}

impl Agg {
    fn observe(&mut self, ts: &str) {
        if self.count == 0 || ts < self.first.as_str() {
            self.first = ts.to_string();
        }
        if ts > self.last.as_str() {
            self.last = ts.to_string();
        }
        self.count += 1;
    }
}

/// Aggregate parsed records into the report shape the session page renders.
#[must_use]
pub fn report_from_records(records: &[Decision]) -> Value {
    let mut timeline: Vec<(String, i64)> = records
        .iter()
        .filter(|r| r.kind == Kind::Enter)
        .map(|r| (r.ts.clone(), r.step.unwrap_or(0)))
        .collect();
    timeline.sort_by(|a, b| a.0.cmp(&b.0));

    let mut denied_tools: BTreeMap<(i64, String, String), Agg> = BTreeMap::new();
    let mut denied_hosts: BTreeMap<(i64, String), Agg> = BTreeMap::new();
    let mut transitions: Vec<Value> = Vec::new();

    for r in records {
        match r.kind {
            Kind::Check if r.verdict == "deny" => {
                let key =
                    (r.step.unwrap_or(0), r.tool.clone().unwrap_or_default(), r.target.clone());
                denied_tools.entry(key).or_default().observe(&r.ts);
            }
            Kind::Network if r.verdict == "deny" => {
                let step = r.step.or_else(|| step_at(&timeline, &r.ts)).unwrap_or(0);
                denied_hosts.entry((step, r.target.clone())).or_default().observe(&r.ts);
            }
            Kind::Transition => transitions.push(json!({
                "ts": r.ts,
                "step": r.step,
                "target": r.target,
                "verdict": r.verdict,
                "detail": r.rule,
            })),
            _ => {}
        }
    }

    let denied_tools: Vec<Value> = denied_tools
        .into_iter()
        .map(|((step, tool, target), a)| {
            json!({
                "step": step,
                "tool": tool,
                "target": target,
                "count": a.count,
                "first_ts": a.first,
                "last_ts": a.last,
            })
        })
        .collect();
    let denied_hosts: Vec<Value> = denied_hosts
        .into_iter()
        .map(|((step, host), a)| {
            json!({
                "step": step,
                "host": host,
                "count": a.count,
                "first_ts": a.first,
                "last_ts": a.last,
            })
        })
        .collect();

    let steps = step_durations(&timeline);

    json!({
        "denied_tools": denied_tools,
        "denied_hosts": denied_hosts,
        "transitions": transitions,
        "steps": steps,
    })
}

/// Time per step from the enter timeline: each entry runs until the next enter.
/// The terminal `Exit` (step `-1`) closes the last real step and is not itself a
/// duration row.
fn step_durations(timeline: &[(String, i64)]) -> Vec<Value> {
    let mut out = Vec::new();
    for i in 0..timeline.len() {
        let (ref entered, step) = timeline[i];
        if step < 0 {
            continue;
        }
        let exited = timeline.get(i + 1).map(|(t, _)| t.clone());
        let seconds = exited.as_ref().and_then(|e| duration_secs(entered, e));
        out.push(json!({
            "step": step,
            "entered_ts": entered,
            "exited_ts": exited,
            "seconds": seconds,
        }));
    }
    out
}

/// Whole seconds between two `now_ts` timestamps, or `None` if either is
/// unparseable.
fn duration_secs(start: &str, end: &str) -> Option<i64> {
    Some(epoch_millis(end)?.saturating_sub(epoch_millis(start)?) / 1000)
}

/// Parse a `now_ts`-shaped `YYYY-MM-DDTHH:MM:SS.mmmZ` back to epoch millis.
fn epoch_millis(ts: &str) -> Option<i64> {
    let (date, rest) = ts.split_once('T')?;
    let rest = rest.strip_suffix('Z')?;
    let mut dparts = date.split('-');
    let y: i64 = dparts.next()?.parse().ok()?;
    let mo: i64 = dparts.next()?.parse().ok()?;
    let d: i64 = dparts.next()?.parse().ok()?;
    let (time, millis) = rest.split_once('.').unwrap_or((rest, "0"));
    let mut tparts = time.split(':');
    let h: i64 = tparts.next()?.parse().ok()?;
    let mi: i64 = tparts.next()?.parse().ok()?;
    let s: i64 = tparts.next()?.parse().ok()?;
    let millis: i64 = millis.parse().ok()?;
    let days = days_from_civil(y, mo, d);
    Some(((days * 86_400 + h * 3600 + mi * 60 + s) * 1000) + millis)
}

/// `(year, month, day)` → days since the Unix epoch (inverse of
/// [`civil_from_days`]).
#[allow(clippy::many_single_char_names, clippy::similar_names)]
const fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamp_roundtrips_through_epoch() {
        let ts = now_ts();
        let ms = epoch_millis(&ts).unwrap();
        let system_ms =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis()
                as i64;
        assert!((system_ms - ms).abs() < 5000, "ts={ts} ms={ms} sys={system_ms}");
    }

    #[test]
    fn known_epoch_millis() {
        // 2026-07-27T00:00:00.000Z
        assert_eq!(epoch_millis("2026-07-27T00:00:00.000Z"), Some(1_785_110_400_000));
    }

    #[test]
    fn disabled_log_is_noop() {
        let log = DecisionLog::new(None);
        assert!(!log.is_enabled());
        log.check(1, "Bash", "rm -rf /", false, "denied");
    }

    #[test]
    fn appends_and_parses_lines() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("d.jsonl");
        let log = DecisionLog::new(Some(path.clone()));
        log.enter(1);
        log.check(1, "Bash", "git push", false, "'git push' disallowed");
        log.check(1, "Read", "/x", true, "");
        let records = parse_log(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(records.len(), 3);
        assert_eq!(records[1].verdict, "deny");
        assert_eq!(records[1].tool.as_deref(), Some("Bash"));
    }

    #[test]
    fn report_dedupes_and_counts_denied_tools() {
        let records = vec![
            Decision {
                ts: "2026-07-27T10:00:00.000Z".into(),
                source: Source::Guard,
                kind: Kind::Enter,
                step: Some(1),
                tool: None,
                target: String::new(),
                verdict: "enter".into(),
                rule: String::new(),
            },
            deny_check("2026-07-27T10:00:01.000Z", 1, "Bash", "git push"),
            deny_check("2026-07-27T10:00:05.000Z", 1, "Bash", "git push"),
            deny_check("2026-07-27T10:00:09.000Z", 1, "Bash", "curl x"),
        ];
        let report = report_from_records(&records);
        let tools = report["denied_tools"].as_array().unwrap();
        assert_eq!(tools.len(), 2);
        let push = tools.iter().find(|t| t["target"] == "git push").unwrap();
        assert_eq!(push["count"], 2);
        assert_eq!(push["first_ts"], "2026-07-27T10:00:01.000Z");
        assert_eq!(push["last_ts"], "2026-07-27T10:00:05.000Z");
    }

    #[test]
    fn network_denials_attribute_to_active_step_and_time() {
        let records = vec![
            enter("2026-07-27T10:00:00.000Z", 1),
            net_deny("2026-07-27T10:00:03.000Z", "github.com:443"),
            enter("2026-07-27T10:00:10.000Z", 2),
            net_deny("2026-07-27T10:00:12.000Z", "github.com:443"),
            net_deny("2026-07-27T10:00:13.000Z", "github.com:443"),
            enter("2026-07-27T10:00:20.000Z", -1),
        ];
        let report = report_from_records(&records);
        let hosts = report["denied_hosts"].as_array().unwrap();
        // (step1, github) once and (step2, github) twice — two rows.
        assert_eq!(hosts.len(), 2);
        let s2 = hosts.iter().find(|h| h["step"] == 2).unwrap();
        assert_eq!(s2["count"], 2);

        let steps = report["steps"].as_array().unwrap();
        assert_eq!(steps.len(), 2, "exit step is not a duration row");
        assert_eq!(steps[0]["step"], 1);
        assert_eq!(steps[0]["seconds"], 10);
        assert_eq!(steps[1]["step"], 2);
        assert_eq!(steps[1]["seconds"], 10);
    }

    fn deny_check(ts: &str, step: i64, tool: &str, target: &str) -> Decision {
        Decision {
            ts: ts.into(),
            source: Source::Guard,
            kind: Kind::Check,
            step: Some(step),
            tool: Some(tool.into()),
            target: target.into(),
            verdict: "deny".into(),
            rule: "denied".into(),
        }
    }

    fn enter(ts: &str, step: i64) -> Decision {
        Decision {
            ts: ts.into(),
            source: Source::Guard,
            kind: Kind::Enter,
            step: Some(step),
            tool: None,
            target: String::new(),
            verdict: "enter".into(),
            rule: String::new(),
        }
    }

    fn net_deny(ts: &str, host: &str) -> Decision {
        Decision {
            ts: ts.into(),
            source: Source::Proxy,
            kind: Kind::Network,
            step: None,
            tool: None,
            target: host.into(),
            verdict: "deny".into(),
            rule: "not in allow-list".into(),
        }
    }
}
