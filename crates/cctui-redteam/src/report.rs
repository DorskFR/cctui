use serde::{Deserialize, Serialize};

use crate::oracle::{self, Verdict};
use crate::probe::ProbeResult;
use crate::sink::SinkHit;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScenarioReport {
    pub id: String,
    pub class: String,
    pub verdict: Verdict,
    pub probes: Vec<ProbeResult>,
    pub sink_hits: Vec<SinkHit>,
}

impl ScenarioReport {
    #[must_use]
    pub fn judge(
        id: String,
        class: String,
        probes: Vec<ProbeResult>,
        sink_hits: Vec<SinkHit>,
    ) -> Self {
        let verdict = oracle::classify(&probes, &sink_hits);
        Self { id, class, verdict, probes, sink_hits }
    }

    #[must_use]
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_owned())
    }

    #[must_use]
    pub fn to_markdown(&self) -> String {
        use std::fmt::Write as _;
        let mut out = format!(
            "# red-team scenario {}\n\n- class: {}\n- verdict: **{}**\n\n## probes\n\n| id | verdict | detail |\n|---|---|---|\n",
            self.id,
            self.class,
            self.verdict.as_str()
        );
        for p in &self.probes {
            let verdict = serde_json::to_value(p.verdict)
                .ok()
                .and_then(|v| v.as_str().map(ToOwned::to_owned))
                .unwrap_or_default();
            let _ = writeln!(out, "| {} | {verdict} | {} |", p.id, p.detail);
        }
        if !self.sink_hits.is_empty() {
            out.push_str("\n## sink hits\n\n");
            for h in &self.sink_hits {
                let _ = writeln!(out, "- {} {} {}", h.peer, h.token, h.detail);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::ProbeVerdict;

    #[test]
    fn a_contained_scenario_renders_its_verdict_and_every_probe_row() {
        let probes = vec![
            ProbeResult {
                id: "EGR-01".to_owned(),
                verdict: ProbeVerdict::Blocked,
                detail: "no response".to_owned(),
            },
            ProbeResult {
                id: "EGR-03".to_owned(),
                verdict: ProbeVerdict::Skipped,
                detail: "no dig".to_owned(),
            },
        ];
        let report = ScenarioReport::judge("egr".to_owned(), "EGR".to_owned(), probes, vec![]);
        assert_eq!(report.verdict, Verdict::Contained);
        let md = report.to_markdown();
        assert!(md.contains("verdict: **contained**"));
        assert!(md.contains("| EGR-01 | blocked | no response |"));
        assert!(md.contains("| EGR-03 | skipped | no dig |"));
        assert!(serde_json::from_str::<ScenarioReport>(&report.to_json()).is_ok());
    }

    #[test]
    fn a_sink_hit_makes_the_report_escape_and_lists_the_hit() {
        let report = ScenarioReport::judge(
            "egr".to_owned(),
            "EGR".to_owned(),
            vec![ProbeResult {
                id: "EGR-01".to_owned(),
                verdict: ProbeVerdict::Blocked,
                detail: String::new(),
            }],
            vec![SinkHit::tcp("10.0.0.9:443", "cnry-x")],
        );
        assert_eq!(report.verdict, Verdict::Escaped);
        assert!(report.to_markdown().contains("sink hits"));
    }
}
