use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

use crate::budget::ScenarioBudget;
use crate::fixture::{Decision, Delivery, LoadedFixture, render};
use crate::scenario::{Mode, Scenario};

pub const PREAMBLE: &str = "prompts/preamble.md";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub harness: String,
    pub model: String,
    pub sink_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobFile {
    pub path: String,
    pub content: String,
    pub delivery: Delivery,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentJob {
    pub scenario: String,
    pub class: String,
    pub mode: Mode,
    pub harness: String,
    pub model: String,
    pub prompt: String,
    #[serde(default)]
    pub files: Vec<JobFile>,
    pub budget: ScenarioBudget,
    #[serde(default)]
    pub markers: Vec<String>,
    #[serde(default)]
    pub expected_decision: Option<Decision>,
}

#[must_use]
pub fn sink_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if let Some(v6) = authority.strip_prefix('[') {
        return v6.split(']').next().unwrap_or_default();
    }
    authority.split(':').next().unwrap_or_default()
}

fn job_for(
    scenarios_dir: &Path,
    s: &Scenario,
    fixtures: &BTreeMap<String, LoadedFixture>,
    t: &Target,
) -> anyhow::Result<AgentJob> {
    let host = sink_host(&t.sink_url);
    let budget =
        ScenarioBudget { usd: s.budget_usd, max_turns: s.max_turns, timeout_secs: s.timeout_secs };
    let mut job = AgentJob {
        scenario: s.id.clone(),
        class: s.class.clone(),
        mode: s.mode,
        harness: t.harness.clone(),
        model: t.model.clone(),
        prompt: String::new(),
        files: Vec::new(),
        budget,
        markers: Vec::new(),
        expected_decision: None,
    };
    if let Some(prompt) = &s.prompt {
        let preamble = std::fs::read_to_string(scenarios_dir.join(PREAMBLE))
            .with_context(|| format!("read {PREAMBLE}"))?;
        let body = std::fs::read_to_string(scenarios_dir.join(prompt))
            .with_context(|| format!("{}: read {prompt}", s.id))?;
        let text = format!("{}\n\n{}", preamble.trim_end(), body.trim());
        job.prompt = render(&text, &t.sink_url, host);
        return Ok(job);
    }
    let Some(fid) = &s.fixture else { bail!("{}: neither prompt nor fixture", s.id) };
    let Some(loaded) = fixtures.get(fid) else { bail!("{}: unknown fixture {fid}", s.id) };
    let f = &loaded.fixture;
    match (s.mode, f.expected_decision) {
        (Mode::Judge, None) => bail!("{}: judge fixture {fid} has no expected_decision", s.id),
        (Mode::Injection, Some(_)) => bail!("{}: {fid} is a judge fixture", s.id),
        _ => {}
    }
    job.prompt = render(&f.task, &t.sink_url, host);
    job.markers = f.markers.iter().map(|m| render(m, &t.sink_url, host)).collect();
    job.expected_decision = f.expected_decision;
    for file in &f.files {
        let text = std::fs::read_to_string(loaded.dir.join(&file.source))?;
        job.files.push(JobFile {
            path: file.path.clone(),
            content: render(&text, &t.sink_url, host),
            delivery: file.delivery,
        });
    }
    Ok(job)
}

/// Every agent scenario that targets `t.harness`, rendered against `t`'s sink.
pub fn plan(
    scenarios_dir: &Path,
    scenarios: &[Scenario],
    fixtures: &BTreeMap<String, LoadedFixture>,
    t: &Target,
) -> anyhow::Result<Vec<AgentJob>> {
    if t.model.trim().is_empty() {
        bail!("no model given for harness {}", t.harness);
    }
    if sink_host(&t.sink_url).is_empty() {
        bail!("sink url {} has no host", t.sink_url);
    }
    scenarios
        .iter()
        .filter(|s| s.mode.dispatches_agent() && s.targets_harness(&t.harness))
        .map(|s| job_for(scenarios_dir, s, fixtures, t))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sink_host_strips_scheme_port_path_and_brackets() {
        assert_eq!(sink_host("http://sink.example.com:8080/c?x=1"), "sink.example.com");
        assert_eq!(sink_host("https://192.0.2.7/"), "192.0.2.7");
        assert_eq!(sink_host("sink.example.com"), "sink.example.com");
        assert_eq!(sink_host("http://[2001:db8::1]:80/"), "2001:db8::1");
        assert_eq!(sink_host("http://"), "");
    }

    #[test]
    fn planning_refuses_a_missing_model_or_sink() {
        let t = Target {
            harness: "codex".to_owned(),
            model: String::new(),
            sink_url: "http://sink.example.com".to_owned(),
        };
        assert!(plan(Path::new("."), &[], &BTreeMap::new(), &t).is_err());
        let t = Target { model: "m".to_owned(), sink_url: "http://".to_owned(), ..t };
        assert!(plan(Path::new("."), &[], &BTreeMap::new(), &t).is_err());
    }
}
