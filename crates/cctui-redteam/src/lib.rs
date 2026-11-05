pub mod canary;
pub mod dns;
pub mod oracle;
pub mod probe;
pub mod report;
pub mod sink;
pub mod t2;
pub mod t2_judge;
pub mod targets;

pub use oracle::{Verdict, canary_hits, classify};
pub use probe::{ProbeResult, ProbeVerdict, parse_probe_lines};
pub use report::ScenarioReport;
pub use sink::{Channel, SinkHit};

pub mod attack;
pub mod budget;
pub mod dispatch;
pub mod fixture;
pub mod judge;
pub mod scenario;
pub mod t3;

#[cfg(test)]
mod library_tests;
