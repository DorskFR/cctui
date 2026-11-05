use std::io::Read as _;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use cctui_redteam::report::ScenarioReport;
use cctui_redteam::sink::SinkHit;
use cctui_redteam::{Verdict, parse_probe_lines};

#[derive(Parser)]
#[command(name = "cctui-redteam", about = "Red-team probe oracle and sink")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the sink: an HTTP + raw-TCP listener that records every connection.
    Sink {
        #[arg(long, default_value = "0.0.0.0:8080")]
        http: String,
        #[arg(long, default_value = "0.0.0.0:9090")]
        tcp: String,
    },
    /// Run the DNS sink: authoritative for `--zone` over UDP, hit log on `--http`.
    DnsSink {
        #[arg(long, default_value = "0.0.0.0:5353")]
        listen: String,
        #[arg(long, default_value = "0.0.0.0:8053")]
        http: String,
        #[arg(long)]
        zone: String,
        /// Address returned for in-zone `A` queries; none = empty answers.
        #[arg(long)]
        answer: Option<std::net::Ipv4Addr>,
    },
    /// Phase-3 live-deployment run: plan, targets and judge.
    #[command(subcommand)]
    T2(cctui_redteam::t2::Command),
    /// Classify one scenario from collected probe output and sink hits.
    Judge {
        #[arg(long)]
        id: String,
        #[arg(long, default_value = "UNK")]
        class: String,
        /// Probe stdout file, or `-` for stdin.
        #[arg(long, default_value = "-")]
        probes: String,
        /// JSON array of sink hits (the sink's `/hits` body); absent = none.
        #[arg(long)]
        sink_hits: Option<PathBuf>,
        /// Directory for `report.{json,md}`.
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Phase-4 agent scenarios: check, plan, run and judge.
    #[command(subcommand)]
    T3(cctui_redteam::t3::Command),
}

fn read_input(spec: &str) -> anyhow::Result<String> {
    if spec == "-" {
        let mut s = String::new();
        std::io::stdin().read_to_string(&mut s)?;
        Ok(s)
    } else {
        Ok(std::fs::read_to_string(spec)?)
    }
}

fn load_sink_hits(path: Option<&PathBuf>) -> anyhow::Result<Vec<SinkHit>> {
    let Some(path) = path else { return Ok(Vec::new()) };
    let text = std::fs::read_to_string(path)?;
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    Ok(serde_json::from_str(&text)?)
}

fn run_judge(
    id: String,
    class: String,
    probes: &str,
    sink_hits: Option<&PathBuf>,
    out: Option<&PathBuf>,
) -> anyhow::Result<Verdict> {
    let probe_results = parse_probe_lines(&read_input(probes)?);
    let hits = load_sink_hits(sink_hits)?;
    let report = ScenarioReport::judge(id, class, probe_results, hits);
    if let Some(dir) = out {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("report.json"), report.to_json())?;
        std::fs::write(dir.join("report.md"), report.to_markdown())?;
    }
    println!("{}", report.to_markdown());
    Ok(report.verdict)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Sink { http, tcp } => cctui_redteam::sink::run(&http, &tcp).await,
        Command::DnsSink { listen, http, zone, answer } => {
            cctui_redteam::dns::run(&listen, &http, &zone, answer).await
        }
        Command::T2(cmd) => {
            if cctui_redteam::t2::cli(cmd)? {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
        Command::Judge { id, class, probes, sink_hits, out } => {
            let verdict = run_judge(id, class, &probes, sink_hits.as_ref(), out.as_ref())?;
            if verdict.is_pass() {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
        Command::T3(cmd) => {
            if cctui_redteam::t3::cli(cmd).await? {
                Ok(())
            } else {
                std::process::exit(1);
            }
        }
    }
}
