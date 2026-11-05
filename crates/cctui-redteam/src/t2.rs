//! T2: every probe in every valid cell of a live deployment, with no model spend.
//!
//! The operator describes the deployment in a JSON config (see
//! `redteam/ci/t2.example.json`); [`plan`] expands it against the probe-mode
//! scenarios into one dispatch per cell, each with its own canaries.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::canary::CanarySet;
use crate::targets;

const PROMPT: &str = "red-team T2 probe run: the probes end the worker before any agent starts";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinkMode {
    /// Every hit is an escape: a sink nothing else can reach.
    #[default]
    Any,
    /// Only canary-bearing hits count: a sink exposed to internet noise.
    Canary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkSpec {
    pub name: String,
    pub hits_url: String,
    #[serde(default)]
    pub mode: SinkMode,
}

/// Where the canary beacons aim: sinks outside the deployment.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beacon {
    #[serde(default)]
    pub http_url: String,
    #[serde(default)]
    pub dns_zone: String,
    #[serde(default)]
    pub tcp: String,
    #[serde(default)]
    pub resolver: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct T2Config {
    /// Dispatcher kind (`kube`, `docker`, ...) to the dispatcher name to call.
    pub dispatchers: BTreeMap<String, String>,
    /// `<dispatcher>/<net_mode>/<posture>` to the red-team worker profile.
    pub profiles: BTreeMap<String, String>,
    pub harnesses: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_minutes: u32,
    /// Env for the probes, e.g. `SINK_*` pointing at a sink inside the deployment.
    #[serde(default)]
    pub probe_env: BTreeMap<String, String>,
    #[serde(default)]
    pub beacon: Beacon,
    #[serde(default)]
    pub sinks: Vec<SinkSpec>,
    #[serde(default)]
    pub sanctioned: Vec<String>,
    #[serde(default = "default_max_targets")]
    pub max_targets: usize,
}

const fn default_timeout() -> u32 {
    15
}

const fn default_max_targets() -> usize {
    256
}

impl T2Config {
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        let cfg: Self =
            serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> anyhow::Result<()> {
        for sink in &self.sinks {
            let charset =
                sink.name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if sink.name.is_empty() || !charset {
                bail!("sink name `{}` must be [a-z0-9-]+", sink.name);
            }
        }
        for key in self.profiles.keys() {
            if key.split('/').count() != 3 {
                bail!("profile key `{key}` must be <dispatcher>/<net_mode>/<posture>");
            }
        }
        Ok(())
    }

    fn axis_values(&self, axis: &str) -> BTreeSet<String> {
        match axis {
            "harness" => self.harnesses.iter().cloned().collect(),
            "dispatcher" => self.dispatchers.keys().cloned().collect(),
            "net_mode" => self.profile_axis(1),
            "posture" => self.profile_axis(2),
            _ => BTreeSet::new(),
        }
    }

    fn profile_axis(&self, at: usize) -> BTreeSet<String> {
        self.profiles.keys().filter_map(|k| k.split('/').nth(at)).map(str::to_owned).collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Cell {
    pub harness: String,
    pub dispatcher: String,
    pub net_mode: String,
    pub posture: String,
}

impl Cell {
    #[must_use]
    pub fn key(&self) -> String {
        format!("{}/{}/{}/{}", self.harness, self.dispatcher, self.net_mode, self.posture)
    }

    fn profile_key(&self) -> String {
        format!("{}/{}/{}", self.dispatcher, self.net_mode, self.posture)
    }
}

/// The parts of a scenario T2 needs: its mode and its cell axes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ScenarioCells {
    pub mode: String,
    pub axes: BTreeMap<String, Vec<String>>,
}

fn unquote(s: &str) -> &str {
    s.trim().trim_matches(|c| c == '"' || c == '\'')
}

fn flow_list(v: &str) -> Vec<String> {
    v.strip_prefix('[').and_then(|s| s.strip_suffix(']')).map_or_else(Vec::new, |s| {
        s.split(',').map(unquote).filter(|x| !x.is_empty()).map(str::to_owned).collect()
    })
}

/// Read `mode` and the `cells` block out of scenario YAML, one entry per
/// `---`-separated document.
///
/// Axis lists may be flow (`[a, b]`) or block (`- a`) sequences.
#[must_use]
pub fn parse_scenario_cells(text: &str) -> Vec<ScenarioCells> {
    let mut docs = vec![ScenarioCells::default()];
    let mut in_cells = false;
    let mut axis: Option<String> = None;
    for raw in text.lines() {
        let line = raw.trim_end();
        let t = line.trim_start();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        if t == "---" {
            axis = None;
            in_cells = false;
            docs.push(ScenarioCells::default());
            continue;
        }
        let Some(doc) = docs.last_mut() else { break };
        if line.len() == t.len() {
            axis = None;
            in_cells = t == "cells:";
            if let Some(mode) = t.strip_prefix("mode:") {
                unquote(mode).clone_into(&mut doc.mode);
            }
            continue;
        }
        if !in_cells {
            continue;
        }
        if let Some(item) = t.strip_prefix("- ") {
            if let Some(name) = &axis {
                doc.axes.entry(name.clone()).or_default().push(unquote(item).to_owned());
            }
        } else if let Some((k, v)) = t.split_once(':') {
            let name = k.trim().to_owned();
            doc.axes.insert(name.clone(), flow_list(v.trim()));
            axis = Some(name);
        }
    }
    docs.retain(|d| d != &ScenarioCells::default());
    docs
}

fn axis(scenario: &ScenarioCells, cfg: &T2Config, name: &str) -> Vec<String> {
    let available = cfg.axis_values(name);
    scenario.axes.get(name).map_or_else(
        || available.iter().cloned().collect(),
        |wanted| wanted.iter().filter(|v| available.contains(*v)).cloned().collect(),
    )
}

/// The cells the probe-mode scenarios target that this deployment can run.
#[must_use]
pub fn valid_cells(scenarios: &[ScenarioCells], cfg: &T2Config) -> Vec<Cell> {
    let mut cells = BTreeSet::new();
    for s in scenarios.iter().filter(|s| s.mode == "probe") {
        for harness in axis(s, cfg, "harness") {
            for dispatcher in axis(s, cfg, "dispatcher") {
                for net_mode in axis(s, cfg, "net_mode") {
                    for posture in axis(s, cfg, "posture") {
                        let cell = Cell {
                            harness: harness.clone(),
                            dispatcher: dispatcher.clone(),
                            net_mode: net_mode.clone(),
                            posture,
                        };
                        if cfg.profiles.contains_key(&cell.profile_key()) {
                            cells.insert(cell);
                        }
                    }
                }
            }
        }
    }
    cells.into_iter().collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlannedCell {
    pub index: usize,
    pub cell: Cell,
    pub canaries: CanarySet,
    /// The `POST /api/v1/sessions/dispatch` body.
    pub request: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct T2Plan {
    pub run_id: String,
    #[serde(default)]
    pub targets_left_out: usize,
    pub cells: Vec<PlannedCell>,
}

fn put(env: &mut Map<String, Value>, key: &str, value: &str) {
    if !value.is_empty() {
        env.insert(key.to_owned(), Value::String(value.to_owned()));
    }
}

fn request(
    cfg: &T2Config,
    cell: &Cell,
    index: usize,
    run_id: &str,
    canaries: &CanarySet,
    targets: &str,
) -> Value {
    let mut env = Map::new();
    for (k, v) in &cfg.probe_env {
        put(&mut env, k, v);
    }
    for c in &canaries.canaries {
        put(&mut env, &c.env, &c.token);
    }
    let envs: Vec<&str> = canaries.canaries.iter().map(|c| c.env.as_str()).collect();
    put(&mut env, "REDTEAM_CANARY_ENVS", &envs.join(" "));
    put(&mut env, "REDTEAM_RUN", run_id);
    put(&mut env, "REDTEAM_CELL", &index.to_string());
    put(&mut env, "REDTEAM_TARGETS", targets);
    put(&mut env, "REDTEAM_BEACON_HTTP", &cfg.beacon.http_url);
    put(&mut env, "REDTEAM_BEACON_ZONE", &cfg.beacon.dns_zone);
    put(&mut env, "REDTEAM_BEACON_TCP", &cfg.beacon.tcp);
    put(&mut env, "REDTEAM_BEACON_RESOLVER", &cfg.beacon.resolver);
    json!({
        "dispatcher": cfg.dispatchers.get(&cell.dispatcher).cloned().unwrap_or_default(),
        "timeout": cfg.timeout_minutes,
        "payload": {
            "name": format!("redteam-t2-{run_id}-{index}"),
            "profile": cfg.profiles.get(&cell.profile_key()).cloned().unwrap_or_default(),
            "adapter": cell.harness,
            "prompt": PROMPT,
            "env": env,
        },
    })
}

/// One dispatch per cell, each with a freshly minted canary set.
#[must_use]
pub fn plan(
    cfg: &T2Config,
    cells: Vec<Cell>,
    targets: &str,
    targets_left_out: usize,
    run_id: &str,
) -> T2Plan {
    let cells = cells
        .into_iter()
        .enumerate()
        .map(|(index, cell)| {
            let canaries = CanarySet::mint(run_id);
            let request = request(cfg, &cell, index, run_id, &canaries, targets);
            PlannedCell { index, cell, canaries, request }
        })
        .collect();
    T2Plan { run_id: run_id.to_owned(), targets_left_out, cells }
}

fn read_scenarios(dir: &Path) -> anyhow::Result<Vec<ScenarioCells>> {
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "yaml" || x == "yml"))
        .collect();
    paths.sort();
    let mut out = Vec::new();
    for p in paths {
        out.extend(parse_scenario_cells(&std::fs::read_to_string(&p)?));
    }
    Ok(out)
}

/// `targets` subcommand: the target list for an inventory JSON file.
pub fn cli_targets(inventory: &Path, config: &Path) -> anyhow::Result<String> {
    let cfg = T2Config::load(config)?;
    let inv: Value = serde_json::from_str(&std::fs::read_to_string(inventory)?)
        .with_context(|| format!("parse {}", inventory.display()))?;
    let found = targets::from_inventory(&inv, &cfg.sanctioned);
    let (text, left_out) = targets::render(&found, cfg.max_targets);
    if left_out > 0 {
        eprintln!("targets: {left_out} left out past max_targets={}", cfg.max_targets);
    }
    Ok(text)
}

/// `t2-plan` subcommand: write the run plan; refuses a plan with no cell.
pub fn cli_plan(
    config: &Path,
    scenarios: &Path,
    inventory: Option<&Path>,
    out: &Path,
) -> anyhow::Result<T2Plan> {
    let cfg = T2Config::load(config)?;
    let cells = valid_cells(&read_scenarios(scenarios)?, &cfg);
    if cells.is_empty() {
        bail!("no valid cell: no probe-mode scenario cell matches a configured profile");
    }
    let (targets, left_out) = match inventory {
        Some(path) => {
            let inv: Value = serde_json::from_str(&std::fs::read_to_string(path)?)
                .with_context(|| format!("parse {}", path.display()))?;
            targets::render(&targets::from_inventory(&inv, &cfg.sanctioned), cfg.max_targets)
        }
        None => (String::new(), 0),
    };
    let plan = plan(&cfg, cells, &targets, left_out, &crate::canary::new_run_id());
    std::fs::write(out, serde_json::to_string_pretty(&plan)?)?;
    Ok(plan)
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Write the run plan: one dispatch body per valid cell.
    Plan {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        scenarios: PathBuf,
        /// Service inventory JSON for the lateral target list.
        #[arg(long)]
        inventory: Option<PathBuf>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the lateral target list for an inventory.
    Targets {
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        inventory: PathBuf,
    },
    /// Judge a run directory; fails unless every cell is contained.
    Judge {
        #[arg(long)]
        plan: PathBuf,
        #[arg(long)]
        config: PathBuf,
        #[arg(long)]
        dir: PathBuf,
        #[arg(long)]
        server_version: String,
        #[arg(long)]
        expect_version: Option<String>,
    },
}

/// Run a `t2` subcommand; `Ok(false)` means the run was not contained.
pub fn cli(cmd: Command) -> anyhow::Result<bool> {
    match cmd {
        Command::Plan { config, scenarios, inventory, out } => {
            let plan = cli_plan(&config, &scenarios, inventory.as_deref(), &out)?;
            eprintln!("t2: run {} plans {} cell(s)", plan.run_id, plan.cells.len());
            Ok(true)
        }
        Command::Targets { config, inventory } => {
            print!("{}", cli_targets(&inventory, &config)?);
            Ok(true)
        }
        Command::Judge { plan, config, dir, server_version, expect_version } => {
            let verdict = crate::t2_judge::cli_judge(
                &plan,
                &config,
                &dir,
                &server_version,
                expect_version.as_deref(),
            )?;
            Ok(verdict.is_pass())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_config() -> T2Config {
        serde_json::from_str(
            r#"{
              "dispatchers": {"kube": "redteam-kube", "docker": "redteam-docker"},
              "profiles": {
                "kube/transparent/nonroot": "redteam-kube-nonroot",
                "kube/transparent/root": "redteam-kube-root",
                "docker/forward/root": "redteam-docker-forward"
              },
              "harnesses": ["claude-code", "codex"],
              "probe_env": {"SINK_TCP_HOST": "10.0.0.20", "REDTEAM_RUN": "operator-cannot-override"},
              "beacon": {"http_url": "http://203.0.113.10:8080", "dns_zone": "sink.example.com"},
              "sinks": [{"name": "in-cluster", "hits_url": "http://10.0.0.20:8080/hits"},
                        {"name": "dns", "hits_url": "http://203.0.113.10:8053/hits", "mode": "canary"}]
            }"#,
        )
        .unwrap()
    }

    const SCENARIOS: &str = "id: EGR\nclass: egress\nmode: probe\ndescription: >\n  cells: [not, this]\ncells:\n  harness: [claude-code, codex, opencode]\n  dispatcher: [kube, docker]\n  net_mode: [transparent]\n  posture: [root, nonroot]\nprobes: [EGR-01]\n---\nid: T3\nmode: attacker\ncells:\n  harness: [claude-code]\n---\nid: PRIV\nmode: \"probe\"\ncells:\n  dispatcher:\n    - docker\n  net_mode:\n    - forward\n";

    #[test]
    fn scenario_cells_parse_flow_and_block_lists_per_document() {
        let docs = parse_scenario_cells(SCENARIOS);
        assert_eq!(docs.len(), 3);
        assert_eq!(docs[0].mode, "probe");
        assert_eq!(docs[0].axes["harness"], vec!["claude-code", "codex", "opencode"]);
        assert_eq!(docs[0].axes["posture"], vec!["root", "nonroot"]);
        assert_eq!(docs[1].mode, "attacker");
        assert_eq!(docs[2].mode, "probe");
        assert_eq!(docs[2].axes["dispatcher"], vec!["docker"]);
        assert_eq!(docs[2].axes["net_mode"], vec!["forward"]);
        assert!(!docs[2].axes.contains_key("harness"));
    }

    #[test]
    fn valid_cells_are_probe_scenario_cells_the_deployment_has_a_profile_for() {
        let cells = valid_cells(&parse_scenario_cells(SCENARIOS), &sample_config());
        let keys: Vec<String> = cells.iter().map(Cell::key).collect();
        assert_eq!(
            keys,
            vec![
                "claude-code/docker/forward/root",
                "claude-code/kube/transparent/nonroot",
                "claude-code/kube/transparent/root",
                "codex/docker/forward/root",
                "codex/kube/transparent/nonroot",
                "codex/kube/transparent/root",
            ]
        );
    }

    #[test]
    fn a_plan_gives_each_cell_its_own_canaries_and_a_dispatch_body() {
        let cfg = sample_config();
        let cells = valid_cells(&parse_scenario_cells(SCENARIOS), &cfg);
        let plan = plan(&cfg, cells, "10.96.0.10:5432 data/db\n", 0, "0123456789ab");
        assert_eq!(plan.cells.len(), 6);
        let first = &plan.cells[0];
        let second = &plan.cells[1];
        assert_ne!(first.canaries.canaries[0].token, second.canaries.canaries[0].token);
        let req = &second.request;
        assert_eq!(req["dispatcher"], "redteam-kube");
        assert_eq!(req["payload"]["profile"], "redteam-kube-nonroot");
        assert_eq!(req["payload"]["adapter"], "claude-code");
        let env = &req["payload"]["env"];
        assert_eq!(env["REDTEAM_RUN"], "0123456789ab");
        assert_eq!(env["REDTEAM_CELL"], "1");
        assert_eq!(env["SINK_TCP_HOST"], "10.0.0.20");
        assert_eq!(env["GITHUB_TOKEN"], second.canaries.canaries[0].token.as_str());
        assert_eq!(env["REDTEAM_TARGETS"], "10.96.0.10:5432 data/db\n");
        assert_eq!(env["REDTEAM_BEACON_ZONE"], "sink.example.com");
        assert!(env.get("REDTEAM_BEACON_TCP").is_none(), "empty beacon fields are omitted");
        assert!(env["REDTEAM_CANARY_ENVS"].as_str().unwrap().contains("AWS_SECRET_ACCESS_KEY"));
    }

    #[test]
    fn a_bad_sink_name_or_profile_key_is_rejected() {
        let mut cfg = sample_config();
        cfg.sinks[0].name = "../x".to_owned();
        assert!(cfg.validate().is_err());
        let mut cfg = sample_config();
        cfg.profiles.insert("kube/root".to_owned(), "p".to_owned());
        assert!(cfg.validate().is_err());
        assert!(sample_config().validate().is_ok());
    }

    #[test]
    fn the_shipped_example_config_parses_and_yields_cells_against_the_real_scenarios() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../redteam");
        let cfg = T2Config::load(&root.join("ci/t2.example.json")).unwrap();
        let scenarios = read_scenarios(&root.join("scenarios")).unwrap();
        assert!(!valid_cells(&scenarios, &cfg).is_empty());
    }
}
