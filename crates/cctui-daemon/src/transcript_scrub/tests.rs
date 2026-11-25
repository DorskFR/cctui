use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cctui_crypto::redact::{self, CompiledPatterns};
use cctui_proto::api::privacy_scan::TranscriptAuditReport;
use cctui_proto::ws::{DaemonFrameUp, ScrubPattern, SecretScrubConfig};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::audit::{fingerprint, masked_spans};
use super::rewrite::{clear_stale_tmp, rewrite, tmp_owner};
use super::sweep::{AUDIT_BUSY, AuditGate, audit_mode, detector_key, serve_through, sweeps};
use super::{Harness, IDLE, Ledger, Liveness, Mode, Options, Report, Stamp, Target, run};

const TOKEN: &str = "ghp_ABCDEFGHIJKLMNOPQRSTUVWX0123";
const SESSION: &str = "6f1c2d3e-0000-4000-8000-000000000001";
const CWD: &str = "/home/me/proj";

fn patterns() -> CompiledPatterns {
    redact::compile(true, &[], b"test-key")
}

fn lines() -> Vec<Value> {
    vec![
        json!({"type": "user", "uuid": "u-1", "parentUuid": null, "sessionId": SESSION,
               "cwd": CWD, "timestamp": "2026-11-20T10:00:00.000Z",
               "message": {"role": "user", "content": "deploy it"}}),
        json!({"type": "assistant", "uuid": "u-2", "parentUuid": "u-1", "sessionId": SESSION,
               "cwd": CWD, "timestamp": "2026-11-20T10:00:05.000Z",
               "message": {"role": "assistant", "content": [{"type": "tool_use", "id": "toolu_1",
                   "name": "Bash", "input": {"command": "cat .env"}}]}}),
        json!({"type": "user", "uuid": "u-3", "parentUuid": "u-2", "sessionId": SESSION,
               "cwd": CWD, "timestamp": "2026-11-20T10:00:06.000Z",
               "message": {"role": "user", "content": [{"type": "tool_result",
                   "tool_use_id": "toolu_1",
                   "content": format!("export GH_TOKEN={TOKEN}\nexport REGION=eu")}]}}),
    ]
}

struct Fixture {
    _dir: tempfile::TempDir,
    projects: PathBuf,
    path: PathBuf,
    original: Vec<u8>,
    modified: SystemTime,
}

fn fixture() -> Fixture {
    fixture_with(&lines())
}

fn fixture_with(lines: &[Value]) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    let project = projects.join("-home-me-proj");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION}.jsonl"));
    let mut body = String::new();
    for line in lines {
        body.push_str(&serde_json::to_string(line).unwrap());
        body.push('\n');
    }
    std::fs::write(&path, &body).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let modified = SystemTime::now() - Duration::from_hours(2);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(modified).unwrap();
    Fixture { _dir: dir, projects, path, original: body.into_bytes(), modified }
}

impl Fixture {
    fn target(&self) -> Target {
        Target { path: self.path.clone(), harness: Harness::ClaudeCode }
    }

    fn idle(&self) -> Liveness {
        Liveness::new(self.projects.clone(), SystemTime::now())
    }

    fn pass(&self, mode: Mode, liveness: &Liveness, ledger: Option<&mut Ledger>) -> Report {
        self.pass_with(&patterns(), mode, liveness, ledger)
    }

    fn pass_with(
        &self,
        patterns: &CompiledPatterns,
        mode: Mode,
        liveness: &Liveness,
        ledger: Option<&mut Ledger>,
    ) -> Report {
        let opts = Options { mode, scan_live: true, collect: true, idle: IDLE };
        run(&[self.target()], liveness, patterns, opts, &CancellationToken::new(), ledger)
    }

    fn bytes(&self) -> Vec<u8> {
        std::fs::read(&self.path).unwrap()
    }
}

fn uuids(raw: &[u8]) -> Vec<(Value, Value)> {
    std::str::from_utf8(raw)
        .unwrap()
        .lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("every line stays valid JSON");
            (v["uuid"].clone(), v["parentUuid"].clone())
        })
        .collect()
}

fn leftovers(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".cctui-scrub"))
        .collect()
}

#[test]
fn rewrite_masks_a_tool_result_token_and_keeps_every_line_valid() {
    let fx = fixture();
    let report = fx.pass(Mode::Apply, &fx.idle(), None);
    assert_eq!(report.files_changed, 1, "{report:?}");
    assert_eq!(report.lines_changed, 1);

    let after = fx.bytes();
    let text = String::from_utf8(after.clone()).unwrap();
    assert!(!text.contains(TOKEN), "token left on disk: {text}");
    assert!(text.contains("[REDACTED:github_token"), "no placeholder: {text}");
    assert_eq!(uuids(&after), uuids(&fx.original), "uuid chain changed");

    let before_lens: Vec<usize> = fx.original.split(|b| *b == b'\n').map(<[u8]>::len).collect();
    let after_lens: Vec<usize> = after.split(|b| *b == b'\n').map(<[u8]>::len).collect();
    assert_eq!(before_lens, after_lens, "every line keeps its byte length");

    let meta = std::fs::metadata(&fx.path).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    assert_eq!(meta.modified().unwrap(), fx.modified);
    assert!(leftovers(fx.path.parent().unwrap()).is_empty());
}

#[test]
fn a_file_touched_mid_rewrite_is_skipped() {
    let fx = fixture();
    let stamp = Stamp::of(&fx.path).unwrap();
    let appended = "{\"type\":\"user\",\"uuid\":\"u-4\",\"parentUuid\":\"u-3\"}\n";
    let mut file = std::fs::OpenOptions::new().append(true).open(&fx.path).unwrap();
    std::io::Write::write_all(&mut file, appended.as_bytes()).unwrap();
    drop(file);

    assert!(!rewrite(&fx.path, &patterns(), stamp).unwrap(), "a stale stamp must not swap");
    let mut expected = fx.original.clone();
    expected.extend_from_slice(appended.as_bytes());
    assert_eq!(fx.bytes(), expected, "the harness's append survives untouched");
    assert!(leftovers(fx.path.parent().unwrap()).is_empty());
}

#[test]
fn a_live_session_file_is_never_touched() {
    let fx = fixture();

    let mut registered = fx.idle();
    registered.add_session(SESSION, Some(CWD));
    let mut same_project = fx.idle();
    same_project.add_session("another-live-session", Some(CWD));
    let mut held = fx.idle();
    held.add_open(fx.path.clone());

    for liveness in [registered, same_project, held] {
        let report = fx.pass(Mode::Apply, &liveness, None);
        assert_eq!(report.files_live, 1, "{report:?}");
        assert_eq!(report.files_changed, 0);
        assert_eq!((report.substitutions, report.values_left), (0, 1), "{report:?}");
        assert!(report.by_category.is_empty(), "{report:?}");
        assert_eq!(fx.bytes(), fx.original);
    }

    std::fs::File::options()
        .write(true)
        .open(&fx.path)
        .unwrap()
        .set_modified(SystemTime::now())
        .unwrap();
    let report = fx.pass(Mode::Apply, &fx.idle(), None);
    assert_eq!(report.files_live, 1, "a recently written file counts as live: {report:?}");
    assert_eq!(fx.bytes(), fx.original);
}

#[test]
fn dry_run_changes_nothing_and_reports_masked_findings() {
    let fx = fixture();
    let report = fx.pass(Mode::DryRun, &fx.idle(), None);
    assert_eq!(fx.bytes(), fx.original, "a dry run writes nothing");
    assert!(report.dry_run);
    assert_eq!(report.files_changed, 1);
    assert_eq!(report.by_category.get("github_token"), Some(&1), "{report:?}");

    let finding = report.findings.iter().find(|f| f.category == "github_token").expect("finding");
    assert_eq!(finding.fingerprint, fingerprint(TOKEN));
    assert_eq!(finding.session, SESSION);
    assert_eq!(finding.cwd.as_deref(), Some(CWD));
    assert_eq!(finding.first_seen.as_deref(), Some("2026-11-20T10:00:06.000Z"));
    assert_eq!(finding.occurrences, 1);
    assert!(!report.samples.get("github_token").is_none_or(Vec::is_empty), "{report:?}");

    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains(TOKEN), "the report leaks the value: {serialized}");
    assert!(leftovers(fx.path.parent().unwrap()).is_empty());
}

#[test]
fn a_second_pass_changes_nothing() {
    let fx = fixture();
    fx.pass(Mode::Apply, &fx.idle(), None);
    let once = fx.bytes();
    let again = fx.pass(Mode::Apply, &fx.idle(), None);
    assert_eq!(again.files_changed, 0, "{again:?}");
    assert_eq!(again.substitutions, 0);
    assert_eq!(fx.bytes(), once);
}

#[test]
fn the_ledger_skips_a_settled_file_until_it_changes() {
    let fx = fixture();
    let mut ledger = Ledger::open(None, "k");
    let first = fx.pass(Mode::Apply, &fx.idle(), Some(&mut ledger));
    assert_eq!(first.files_scanned, 1);
    let second = fx.pass(Mode::Apply, &fx.idle(), Some(&mut ledger));
    assert_eq!(second.files_scanned, 0, "settled files are not re-read: {second:?}");
}

#[test]
fn codex_rollouts_are_audited_but_never_rewritten() {
    let fx = fixture();
    let target = Target { path: fx.path.clone(), harness: Harness::Codex };
    let opts = Options { mode: Mode::Apply, scan_live: true, collect: true, idle: IDLE };
    let report = run(&[target], &fx.idle(), &patterns(), opts, &CancellationToken::new(), None);
    assert_eq!(report.files_read_only, 1, "{report:?}");
    assert_eq!(report.files_changed, 0);
    assert_eq!((report.substitutions, report.values_left), (0, 1), "{report:?}");
    assert_eq!(fx.bytes(), fx.original);
}

#[test]
fn masked_spans_recovers_each_value_beside_existing_placeholders() {
    let mut out = Vec::new();
    masked_spans(
        "a=SECRET1 b=[REDACTED:user] c=SECRET2",
        "a=[REDACTED:x:9f2a] b=[REDACTED:user] c=[REDACTED:y]",
        &mut out,
    );
    assert_eq!(
        out,
        vec![("x".to_owned(), "SECRET1".to_owned()), ("y".to_owned(), "SECRET2".to_owned())]
    );
}

#[test]
fn the_server_report_carries_counts_and_masked_samples_only() {
    let fx = fixture();
    let audit = fx.pass(Mode::DryRun, &fx.idle(), None).to_audit();
    assert!(audit.dry_run);
    assert_eq!((audit.files_scanned, audit.files_changed), (1, 1));
    assert_eq!(audit.by_category.get("github_token"), Some(&1));
    assert!(!audit.samples.get("github_token").is_none_or(Vec::is_empty), "{audit:?}");
    let wire = serde_json::to_string(&audit).unwrap();
    assert!(!wire.contains(TOKEN), "the reply leaks the value: {wire}");
    assert!(!wire.contains(SESSION) && !wire.contains(CWD), "{wire}");

    let applied = fx.pass(Mode::Apply, &fx.idle(), None).to_audit();
    assert!(!applied.dry_run);
    assert_eq!(applied.files_changed, 1);
    assert!(!fx.bytes().windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()));
}

const SIGNATURE: &str = "ErUBCkYIBRgCIkDq3kZ8vXw9Lm2NpQa7Tb5Rc1YdW4eFg6HhJj0KkLl8MmNn2Oo4Pp6QqRr8SsTt0UuVv2WwXx4YyZz6AaBbCc8DdEeFfGg0HhIiJjKk2LlMmNnOo4PpQqRrSs6TtUuVvWw8XxYyZzAa0BbCcDdEe";
const REDACTED_DATA: &str = "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpPkNRj2YsvXwC7ZrHjqPuz5JqZ/pbrjs5RVe2YbJ9t";

fn thinking_lines() -> Vec<Value> {
    let mut lines = lines();
    lines.insert(
        2,
        json!({"type": "assistant", "uuid": "u-2b", "parentUuid": "u-2", "sessionId": SESSION,
               "cwd": CWD, "timestamp": "2026-11-20T10:00:05.500Z",
               "message": {"id": "msg_01", "type": "message", "role": "assistant",
                   "model": "claude-opus-5-5", "content": [
                       {"type": "thinking",
                        "thinking": format!("The user exported GH_TOKEN={TOKEN}, deploy with it."),
                        "signature": SIGNATURE},
                       {"type": "redacted_thinking", "data": REDACTED_DATA},
                       {"type": "tool_use", "id": "toolu_2", "name": "Bash",
                        "input": {"command": format!("GH_TOKEN={TOKEN} ./deploy.sh")}}],
                   "stop_reason": "tool_use",
                   "usage": {"input_tokens": 12, "output_tokens": 40}}}),
    );
    lines
}

fn compact(value: &Value) -> String {
    serde_json::to_string(value).unwrap()
}

#[test]
fn thinking_blocks_stay_byte_identical_while_tokens_around_them_are_masked() {
    let lines = thinking_lines();
    let blocks = lines[2]["message"]["content"].as_array().unwrap();
    let (thinking, redacted) = (compact(&blocks[0]), compact(&blocks[1]));
    let mut alone = blocks[0].clone();
    redact::redact_json_stats(&mut alone, &patterns());
    assert_ne!(alone["signature"], SIGNATURE, "the redactor alone would mask this signature");

    let fx = fixture_with(&lines);
    let report = fx.pass(Mode::Apply, &fx.idle(), None);
    assert_eq!(report.files_changed, 1, "{report:?}");
    assert_eq!(report.lines_changed, 2, "tool_use and tool_result lines: {report:?}");

    let after = String::from_utf8(fx.bytes()).unwrap();
    let rows: Vec<&str> = after.lines().collect();
    assert!(rows[2].contains(&thinking), "thinking block changed: {}", rows[2]);
    assert!(rows[2].contains(&redacted), "redacted_thinking block changed: {}", rows[2]);
    let row: Value = serde_json::from_str(rows[2]).unwrap();
    let input = row["message"]["content"][2]["input"]["command"].as_str().unwrap();
    assert!(!input.contains(TOKEN) && input.contains("[REDACTED:"), "{input}");
    assert!(!rows[3].contains(TOKEN), "tool_result token left: {}", rows[3]);
    assert_eq!(uuids(after.as_bytes()), uuids(&fx.original));
}

#[test]
fn hits_on_a_line_that_would_grow_are_left_not_counted_as_masked() {
    let fx = fixture_with(&[json!({"type": "user", "uuid": "u-1", "parentUuid": null,
        "timestamp": "2026-11-20T10:00:00.000Z",
        "message": {"role": "user", "content": "ticket ACME-1234"}})]);
    let patterns =
        redact::compile(true, &[("acme".to_owned(), r"ACME-\d{4}".to_owned())], b"test-key");
    for mode in [Mode::DryRun, Mode::Apply] {
        let report = fx.pass_with(&patterns, mode, &fx.idle(), None);
        assert_eq!(report.lines_unfit, 1, "{report:?}");
        assert_eq!((report.substitutions, report.values_left), (0, 1), "{report:?}");
        assert_eq!((report.files_changed, report.lines_changed), (0, 0), "{report:?}");
        assert!(report.by_category.is_empty(), "{report:?}");
        let audit = report.to_audit();
        assert_eq!((audit.lines_unfit, audit.values_left, audit.substitutions), (1, 1, 0));
        assert_eq!(fx.bytes(), fx.original);
    }
}

#[test]
fn without_a_way_to_list_open_files_every_file_is_audit_only() {
    let fx = fixture();
    let unknown = fx.idle().without_open_files();
    let report = fx.pass(Mode::Apply, &unknown, None);
    assert!(report.open_files_unknown, "{report:?}");
    assert_eq!((report.files_live, report.files_changed), (1, 0), "{report:?}");
    assert_eq!((report.substitutions, report.values_left), (0, 1), "{report:?}");
    assert!(report.to_audit().open_files_unknown);
    assert_eq!(fx.bytes(), fx.original);
    assert!(!fx.pass(Mode::DryRun, &fx.idle(), None).open_files_unknown);
}

#[test]
fn stale_tmp_copies_are_removed_and_nothing_else() {
    let fx = fixture();
    let dir = fx.path.parent().unwrap();
    let own = std::process::id();
    let name = format!("{SESSION}.jsonl");
    let dead = format!(".{name}.999999999.cctui-scrub");
    let mine = format!(".{name}.{own}.cctui-scrub");
    let other_live = format!(".{name}.1.cctui-scrub");
    let not_ours = [".notes.cctui-scrub".to_owned(), format!(".{name}.12x.cctui-scrub")];
    for file in [&dead, &mine, &other_live].into_iter().chain(&not_ours) {
        std::fs::write(dir.join(file), b"copy").unwrap();
    }
    let linked = format!(".{name}.999999998.cctui-scrub");
    std::os::unix::fs::symlink(&fx.path, dir.join(&linked)).unwrap();

    assert_eq!(tmp_owner(&dead), Some(999_999_999));
    assert_eq!(clear_stale_tmp(&fx.projects), 2);
    let mut left = leftovers(dir);
    left.sort();
    let mut expected = vec![other_live, linked];
    expected.extend(not_ours);
    expected.sort();
    assert_eq!(left, expected);
    assert_eq!(fx.bytes(), fx.original);
}

#[test]
fn a_tmp_left_under_this_pid_does_not_block_the_rewrite() {
    let fx = fixture();
    let dir = fx.path.parent().unwrap();
    let stale = dir.join(format!(".{SESSION}.jsonl.{}.cctui-scrub", std::process::id()));
    std::fs::write(&stale, b"left by a killed daemon").unwrap();
    let report = fx.pass(Mode::Apply, &fx.idle(), None);
    assert_eq!((report.files_changed, report.files_failed), (1, 0), "{report:?}");
    assert!(leftovers(dir).is_empty());
    assert!(!fx.bytes().windows(TOKEN.len()).any(|w| w == TOKEN.as_bytes()));
}

#[test]
fn the_ledger_key_moves_with_the_detector_set() {
    let cfg = SecretScrubConfig { enabled: true, local_transcripts: true, ..Default::default() };
    let base = detector_key("1.0.0", &["github_token"], &cfg);
    assert_eq!(base, detector_key("1.0.0", &["github_token"], &cfg));
    assert_ne!(base, detector_key("1.0.1", &["github_token"], &cfg), "a daemon upgrade");
    assert_ne!(base, detector_key("1.0.0", &["github_token", "new_one"], &cfg), "a new detector");
    let user = SecretScrubConfig {
        enabled: true,
        local_transcripts: true,
        patterns: vec![ScrubPattern { name: "acme".into(), regex: "ACME".into() }],
    };
    assert_ne!(base, detector_key("1.0.0", &["github_token"], &user), "a user pattern");

    let fx = fixture();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.json");
    let mut ledger = Ledger::open(Some(path.clone()), &base);
    fx.pass(Mode::Apply, &fx.idle(), Some(&mut ledger));
    ledger.flush();
    let stamp = Stamp::of(&fx.path).unwrap();
    assert!(Ledger::open(Some(path.clone()), &base).settled(&fx.path, stamp));
    let upgraded = detector_key("1.0.1", &["github_token"], &cfg);
    assert!(!Ledger::open(Some(path), &upgraded).settled(&fx.path, stamp), "rescanned");
}

#[test]
fn the_local_sweep_needs_the_main_scrub_switch() {
    let cfg = |enabled, local_transcripts| SecretScrubConfig {
        enabled,
        local_transcripts,
        ..Default::default()
    };
    assert!(sweeps(&cfg(true, true)));
    assert!(!sweeps(&cfg(false, true)), "opt-in without the main scrub");
    assert!(!sweeps(&cfg(true, false)));
    assert_eq!(audit_mode(false, &cfg(true, true)), Mode::Apply);
    assert_eq!(audit_mode(false, &cfg(false, true)), Mode::DryRun, "an apply stays a dry run");
    assert_eq!(audit_mode(true, &cfg(true, true)), Mode::DryRun);
    assert_eq!(audit_mode(true, &cfg(false, false)), Mode::DryRun, "a dry run still scans");
}

#[tokio::test]
async fn one_audit_runs_at_a_time_and_a_same_mode_request_joins_it() {
    static GATE: AuditGate = AuditGate::new();
    let (up, mut frames) = tokio::sync::mpsc::channel(8);
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let audit = |calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
                 wait: Option<std::sync::mpsc::Receiver<()>>| {
        move |dry_run: bool| {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if let Some(wait) = wait {
                wait.recv().unwrap();
            }
            TranscriptAuditReport { dry_run, files_scanned: 7, ..Default::default() }
        }
    };
    let (first, second, third) = (uuid::Uuid::new_v4(), uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
    serve_through(&GATE, first, true, up.clone(), audit(calls.clone(), Some(wait)));
    serve_through(&GATE, second, true, up.clone(), audit(calls.clone(), None));
    serve_through(&GATE, third, false, up.clone(), audit(calls.clone(), None));

    let busy = frames.recv().await.unwrap();
    let DaemonFrameUp::TranscriptAuditResult { request_id, report, error } = busy else {
        panic!("{busy:?}")
    };
    assert_eq!((request_id, report, error.as_deref()), (third, None, Some(AUDIT_BUSY)));

    release.send(()).unwrap();
    let mut answered = Vec::new();
    for _ in 0..2 {
        let frame = frames.recv().await.unwrap();
        let DaemonFrameUp::TranscriptAuditResult { request_id, report, error } = frame else {
            panic!("{frame:?}")
        };
        assert_eq!(error, None);
        assert_eq!(report.map(|r| r.files_scanned), Some(7));
        answered.push(request_id);
    }
    answered.sort();
    let mut expected = vec![first, second];
    expected.sort();
    assert_eq!(answered, expected, "the joined request gets the running result");
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1, "a single pass ran");

    serve_through(&GATE, uuid::Uuid::new_v4(), false, up, audit(calls.clone(), None));
    let frame = frames.recv().await.unwrap();
    assert!(
        matches!(frame, DaemonFrameUp::TranscriptAuditResult { report: Some(_), .. }),
        "the gate opens again once the pass ends: {frame:?}"
    );
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
}
