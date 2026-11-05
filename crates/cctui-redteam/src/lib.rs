pub mod oracle;
pub mod probe;
pub mod report;
pub mod sink;

pub use oracle::{Verdict, canary_hits, classify};
pub use probe::{ProbeResult, ProbeVerdict, parse_probe_lines};
pub use report::ScenarioReport;
pub use sink::{Channel, SinkHit};
