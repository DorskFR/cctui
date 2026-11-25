use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use cctui_crypto::redact::{self, CompiledPatterns};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

use super::audit::{fingerprint, masked_spans};
use super::rewrite::rewrite;
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
    let dir = tempfile::tempdir().unwrap();
    let projects = dir.path().join("projects");
    let project = projects.join("-home-me-proj");
    std::fs::create_dir_all(&project).unwrap();
    let path = project.join(format!("{SESSION}.jsonl"));
    let mut body = String::new();
    for line in lines() {
        body.push_str(&serde_json::to_string(&line).unwrap());
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
        let opts = Options { mode, scan_live: true, collect: true, idle: IDLE };
        run(&[self.target()], liveness, &patterns(), opts, &CancellationToken::new(), ledger)
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
