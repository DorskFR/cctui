use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Deserialize;

use crate::attack::{Target, plan};
use crate::fixture::{Decision, Vector, load_library};
use crate::judge::{Evidence, Expectation, judge};
use crate::oracle::Verdict;
use crate::scenario::{Mode, load_dir};

fn redteam() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("..").join("redteam")
}

fn target(harness: &str) -> Target {
    Target {
        harness: harness.to_owned(),
        model: "test-model".to_owned(),
        sink_url: "http://sink.example.com:8080".to_owned(),
    }
}

#[test]
fn every_scenario_file_loads_and_validates() {
    let all = load_dir(&redteam().join("scenarios")).unwrap();
    for mode in [Mode::Probe, Mode::Attacker, Mode::Injection, Mode::Judge] {
        assert!(all.iter().any(|s| s.mode == mode), "no {mode:?} scenario");
    }
}

#[test]
fn every_fixture_loads_and_every_injection_vector_is_covered() {
    let lib = load_library(&redteam().join("fixtures")).unwrap();
    let vectors: BTreeSet<String> = lib
        .values()
        .filter(|f| f.fixture.expected_decision.is_none())
        .map(|f| format!("{:?}", f.fixture.vector))
        .collect();
    for v in [
        Vector::Repository,
        Vector::IssueComment,
        Vector::ReviewComment,
        Vector::FetchedPage,
        Vector::PackageScript,
        Vector::McpOutput,
        Vector::ContextPack,
        Vector::PeerMessage,
    ] {
        assert!(vectors.contains(&format!("{v:?}")), "no injection fixture for {v:?}");
    }
    assert!(lib.values().any(|f| f.fixture.expected_decision == Some(Decision::Reject)));
}

#[test]
fn every_fixture_is_used_by_a_scenario() {
    let all = load_dir(&redteam().join("scenarios")).unwrap();
    let used: BTreeSet<&str> = all.iter().filter_map(|s| s.fixture.as_deref()).collect();
    for id in load_library(&redteam().join("fixtures")).unwrap().keys() {
        assert!(used.contains(id.as_str()), "fixture {id} is not used by any scenario");
    }
}

#[test]
fn each_attack_class_has_an_attacker_prompt() {
    let all = load_dir(&redteam().join("scenarios")).unwrap();
    for class in ["egress", "lateral", "secrets", "privilege", "guard"] {
        assert!(
            all.iter().any(|s| s.mode == Mode::Attacker && s.class == class),
            "no attacker scenario for {class}"
        );
    }
}

#[test]
fn the_codex_lane_is_driven_by_injection_and_judge_never_the_direct_prompt() {
    let dir = redteam().join("scenarios");
    let all = load_dir(&dir).unwrap();
    let lib = load_library(&redteam().join("fixtures")).unwrap();
    let jobs = plan(&dir, &all, &lib, &target("codex")).unwrap();
    assert!(jobs.iter().all(|j| j.mode != Mode::Attacker));
    assert!(jobs.iter().any(|j| j.mode == Mode::Injection));
    assert!(jobs.iter().any(|j| j.mode == Mode::Judge));
    let claude = plan(&dir, &all, &lib, &target("claude-code")).unwrap();
    assert!(claude.iter().any(|j| j.mode == Mode::Attacker));
}

#[test]
fn planned_jobs_are_fully_rendered_and_budgeted() {
    let dir = redteam().join("scenarios");
    let all = load_dir(&dir).unwrap();
    let lib = load_library(&redteam().join("fixtures")).unwrap();
    for harness in ["claude-code", "codex", "opencode"] {
        for job in plan(&dir, &all, &lib, &target(harness)).unwrap() {
            assert!(job.budget.usd > 0.0 && job.budget.timeout_secs > 0, "{}", job.scenario);
            assert!(!job.prompt.contains("{{"), "{}: unrendered prompt", job.scenario);
            for f in &job.files {
                assert!(!f.content.contains("{{"), "{}: unrendered {}", job.scenario, f.path);
            }
            for m in &job.markers {
                assert!(!m.contains("{{"), "{}: unrendered marker", job.scenario);
            }
            if job.mode == Mode::Attacker {
                assert!(job.prompt.contains("http://sink.example.com:8080"), "{}", job.scenario);
                assert!(job.prompt.contains("Capture a flag and stop"), "{}", job.scenario);
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Transcript {
    expect: Verdict,
    mode: Mode,
    #[serde(default)]
    canaries: Vec<String>,
    #[serde(default)]
    markers: Vec<String>,
    #[serde(default)]
    expected_decision: Option<Decision>,
    #[serde(default)]
    complied: Option<bool>,
    evidence: Evidence,
}

#[test]
fn the_judge_gives_the_expected_verdict_on_every_fixture_transcript() {
    let mut seen = BTreeSet::new();
    let dir = redteam().join("fixtures").join("transcripts");
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.display().to_string();
        let t: Transcript = serde_json::from_str(&std::fs::read_to_string(&path).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let exp = Expectation {
            mode: t.mode,
            canaries: &t.canaries,
            markers: &t.markers,
            expected_decision: t.expected_decision,
        };
        let got = judge(&exp, &t.evidence);
        assert_eq!(got.verdict, t.expect, "{name}: {:?}", got.reasons);
        if t.complied.is_some() {
            assert_eq!(got.complied, t.complied, "{name}: compliance");
        }
        seen.insert(t.expect.as_str());
    }
    for v in ["contained", "escaped", "refused", "inconclusive"] {
        assert!(seen.contains(v), "no fixture transcript expects {v}");
    }
}
