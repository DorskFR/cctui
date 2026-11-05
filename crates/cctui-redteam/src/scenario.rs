//! Scenario files under `redteam/scenarios`: one scenario per YAML document.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

use crate::oracle::Verdict;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Probe,
    Attacker,
    Injection,
    Judge,
}

impl Mode {
    #[must_use]
    pub const fn dispatches_agent(self) -> bool {
        !matches!(self, Self::Probe)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scenario {
    pub id: String,
    pub class: String,
    pub mode: Mode,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub cells: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub probes: Vec<String>,
    /// Attacker prompt, relative to the scenarios directory.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Fixture id under the fixtures directory.
    #[serde(default)]
    pub fixture: Option<String>,
    pub expect: Verdict,
    pub timeout_secs: u64,
    pub budget_usd: f64,
    #[serde(default)]
    pub max_turns: Option<u32>,
}

impl Scenario {
    /// Whether this scenario targets `harness` (no `harness` cell = every harness).
    #[must_use]
    pub fn targets_harness(&self, harness: &str) -> bool {
        self.cells.get("harness").is_none_or(|hs| hs.iter().any(|h| h == harness))
    }

    /// Check the fields a scenario of this mode needs.
    pub fn validate(&self) -> anyhow::Result<()> {
        if self.id.trim().is_empty() || self.class.trim().is_empty() {
            bail!("scenario needs a non-empty id and class");
        }
        if self.expect != Verdict::Contained {
            bail!("{}: expect must be `contained`", self.id);
        }
        if self.timeout_secs == 0 {
            bail!("{}: timeout_secs must be positive", self.id);
        }
        if !self.budget_usd.is_finite() || self.budget_usd < 0.0 {
            bail!("{}: budget_usd must be a finite non-negative number", self.id);
        }
        match self.mode {
            Mode::Probe => {
                if self.probes.is_empty() || self.prompt.is_some() || self.fixture.is_some() {
                    bail!("{}: a probe scenario lists probes and nothing else", self.id);
                }
                if self.budget_usd > 0.0 {
                    bail!("{}: a probe scenario spends no model budget", self.id);
                }
            }
            Mode::Attacker => {
                if self.prompt.is_none() || self.fixture.is_some() {
                    bail!("{}: an attacker scenario needs a prompt and no fixture", self.id);
                }
            }
            Mode::Injection | Mode::Judge => {
                if self.fixture.is_none() || self.prompt.is_some() {
                    bail!("{}: an injection/judge scenario needs a fixture and no prompt", self.id);
                }
            }
        }
        if self.mode.dispatches_agent() && self.budget_usd <= 0.0 {
            bail!("{}: an agent scenario needs a positive budget_usd", self.id);
        }
        Ok(())
    }
}

/// Parse every YAML document in `text` as a scenario.
pub fn parse_documents(text: &str) -> anyhow::Result<Vec<Scenario>> {
    let mut out = Vec::new();
    for doc in serde_norway::Deserializer::from_str(text) {
        out.push(Scenario::deserialize(doc)?);
    }
    Ok(out)
}

fn yaml_files(dir: &Path, out: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let path = entry?.path();
        if path.is_dir() {
            yaml_files(&path, out)?;
        } else if matches!(path.extension().and_then(OsStr::to_str), Some("yaml" | "yml")) {
            out.push(path);
        }
    }
    Ok(())
}

/// Load and validate every scenario under `dir`; ids must be unique.
pub fn load_dir(dir: &Path) -> anyhow::Result<Vec<Scenario>> {
    let mut files = Vec::new();
    yaml_files(dir, &mut files)?;
    files.sort();
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(&file)?;
        let docs = parse_documents(&text).with_context(|| format!("parse {}", file.display()))?;
        for s in docs {
            s.validate().with_context(|| format!("in {}", file.display()))?;
            if let Some(prompt) = &s.prompt
                && !dir.join(prompt).is_file()
            {
                bail!("{}: prompt {prompt} not found under {}", s.id, dir.display());
            }
            if !seen.insert(s.id.clone()) {
                bail!("duplicate scenario id {}", s.id);
            }
            out.push(s);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROBE: &str = "id: EGR\nclass: egress\nmode: probe\nprobes: [EGR-01]\nexpect: contained\ntimeout_secs: 120\nbudget_usd: 0\n";

    #[test]
    fn a_probe_scenario_with_an_integer_budget_parses_and_validates() {
        let docs = parse_documents(PROBE).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].mode, Mode::Probe);
        docs[0].validate().unwrap();
        assert!(docs[0].targets_harness("codex"));
    }

    #[test]
    fn several_documents_in_one_file_each_become_a_scenario() {
        let text = format!(
            "{PROBE}---\nid: INJ-01\nclass: injection\nmode: injection\nfixture: f\ncells:\n  harness: [codex]\nexpect: contained\ntimeout_secs: 60\nbudget_usd: 1.5\n"
        );
        let docs = parse_documents(&text).unwrap();
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[1].fixture.as_deref(), Some("f"));
        assert!(docs[1].targets_harness("codex"));
        assert!(!docs[1].targets_harness("claude-code"));
        docs[1].validate().unwrap();
    }

    #[test]
    fn an_unknown_field_is_rejected() {
        let text = format!("{PROBE}surprise: 1\n");
        assert!(parse_documents(&text).is_err());
    }

    #[test]
    fn mode_specific_fields_are_enforced() {
        let mut s = parse_documents(PROBE).unwrap().remove(0);
        s.budget_usd = 1.0;
        assert!(s.validate().is_err());
        s.budget_usd = 0.0;
        s.mode = Mode::Attacker;
        assert!(s.validate().is_err(), "attacker without prompt");
        s.probes.clear();
        s.prompt = Some("prompts/EGR.md".to_owned());
        assert!(s.validate().is_err(), "attacker without budget");
        s.budget_usd = 2.0;
        s.validate().unwrap();
        s.expect = Verdict::Refused;
        assert!(s.validate().is_err(), "expect must be contained");
    }
}
