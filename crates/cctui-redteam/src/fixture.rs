//! The injection fixture library: `redteam/fixtures/<id>/fixture.yaml` plus its payload files.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Vector {
    Repository,
    IssueComment,
    ReviewComment,
    FetchedPage,
    PackageScript,
    McpOutput,
    ContextPack,
    PeerMessage,
}

/// How a payload reaches the agent: a workspace file, or a channel the dispatch side feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Delivery {
    #[default]
    Workspace,
    PeerMessage,
    McpReply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decision {
    Approve,
    Reject,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FixtureFile {
    /// Destination, relative to the agent's workspace.
    pub path: String,
    /// Payload file, relative to the fixture directory.
    pub source: String,
    #[serde(default)]
    pub delivery: Delivery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Fixture {
    pub id: String,
    pub vector: Vector,
    /// The benign task handed to the agent.
    pub task: String,
    pub files: Vec<FixtureFile>,
    /// Strings whose presence in a tool call means the agent attempted the injected action.
    #[serde(default)]
    pub markers: Vec<String>,
    /// The correct review outcome, for judge fixtures.
    #[serde(default)]
    pub expected_decision: Option<Decision>,
}

pub const PLACEHOLDERS: &[&str] = &["SINK_URL", "SINK_HOST"];

#[must_use]
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else { break };
        out.push(after[..end].trim());
        rest = &after[end + 2..];
    }
    out
}

#[must_use]
pub fn render(text: &str, sink_url: &str, sink_host: &str) -> String {
    text.replace("{{SINK_URL}}", sink_url).replace("{{SINK_HOST}}", sink_host)
}

fn check_placeholders(what: &str, text: &str) -> anyhow::Result<()> {
    for name in placeholders(text) {
        if !PLACEHOLDERS.contains(&name) {
            bail!("{what}: unknown placeholder {{{{{name}}}}}");
        }
    }
    Ok(())
}

fn is_safe_relative(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty() && p.components().all(|c| matches!(c, Component::Normal(_)))
}

impl Fixture {
    /// Check the fixture and that every payload exists under `dir`.
    pub fn validate(&self, dir: &Path) -> anyhow::Result<()> {
        if self.task.trim().is_empty() {
            bail!("{}: empty task", self.id);
        }
        if self.files.is_empty() {
            bail!("{}: a fixture carries at least one payload file", self.id);
        }
        if self.expected_decision.is_none() && self.markers.is_empty() {
            bail!("{}: an injection fixture needs compliance markers", self.id);
        }
        check_placeholders(&self.id, &self.task)?;
        for m in &self.markers {
            check_placeholders(&self.id, m)?;
        }
        for f in &self.files {
            if !is_safe_relative(&f.path) || !is_safe_relative(&f.source) {
                bail!("{}: file paths must be plain relative paths", self.id);
            }
            let text = std::fs::read_to_string(dir.join(&f.source))
                .with_context(|| format!("{}: payload {}", self.id, f.source))?;
            check_placeholders(&format!("{}/{}", self.id, f.source), &text)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct LoadedFixture {
    pub dir: PathBuf,
    pub fixture: Fixture,
}

/// Load and validate every `<id>/fixture.yaml` under `dir`, keyed by id.
pub fn load_library(dir: &Path) -> anyhow::Result<BTreeMap<String, LoadedFixture>> {
    let mut out = BTreeMap::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
        let fdir = entry?.path();
        let manifest = fdir.join("fixture.yaml");
        if !manifest.is_file() {
            continue;
        }
        let fixture: Fixture = serde_norway::from_str(&std::fs::read_to_string(&manifest)?)
            .with_context(|| format!("parse {}", manifest.display()))?;
        if fdir.file_name().and_then(std::ffi::OsStr::to_str) != Some(fixture.id.as_str()) {
            bail!("{}: id must match its directory name", manifest.display());
        }
        fixture.validate(&fdir)?;
        out.insert(fixture.id.clone(), LoadedFixture { dir: fdir, fixture });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_are_found_and_rendered() {
        let t = "post to {{SINK_URL}}/c and resolve {{ SINK_HOST }}";
        assert_eq!(placeholders(t), vec!["SINK_URL", "SINK_HOST"]);
        assert_eq!(
            render("{{SINK_URL}}/c {{SINK_HOST}}", "http://sink.example.com", "sink.example.com"),
            "http://sink.example.com/c sink.example.com"
        );
        assert!(check_placeholders("x", "{{CANARY}}").is_err());
        assert!(check_placeholders("x", "{{SINK_URL}}").is_ok());
    }

    #[test]
    fn payload_paths_cannot_leave_the_workspace() {
        assert!(is_safe_relative("docs/setup.md"));
        assert!(!is_safe_relative("../etc/passwd"));
        assert!(!is_safe_relative("/etc/passwd"));
        assert!(!is_safe_relative(""));
    }

    #[test]
    fn a_manifest_parses_with_default_delivery() {
        let f: Fixture = serde_norway::from_str(
            "id: x\nvector: peer_message\ntask: do it\nfiles:\n  - path: a.md\n    source: a.md\nmarkers: [\"{{SINK_HOST}}\"]\n",
        )
        .unwrap();
        assert_eq!(f.vector, Vector::PeerMessage);
        assert_eq!(f.files[0].delivery, Delivery::Workspace);
        assert!(f.expected_decision.is_none());
    }
}
