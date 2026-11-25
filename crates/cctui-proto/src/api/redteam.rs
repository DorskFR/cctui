//! `/api/v1/redteam/runs` and `/api/v1/admin/redteam`: red-team run results.

use serde::{Deserialize, Serialize};
#[cfg(test)]
use ts_rs::TS;

pub const REDTEAM_REPORT_SCHEMA: u32 = 1;
pub const REDTEAM_MAX_REPORT_BYTES: usize = 256 * 1024;
pub const REDTEAM_MAX_CELLS: usize = 1024;
pub const REDTEAM_MAX_PROBES: usize = 256;
pub const REDTEAM_MAX_REASONS: usize = 32;
pub const REDTEAM_MAX_TEXT: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum RedteamTier {
    T2,
    T3,
}

impl RedteamTier {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::T2 => "t2",
            Self::T3 => "t3",
        }
    }
}

/// Probes report `blocked`, `escaped` or `skipped`; runs and cells the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum RedteamOutcome {
    Contained,
    Escaped,
    Refused,
    Inconclusive,
    Blocked,
    Skipped,
}

impl RedteamOutcome {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Contained => "contained",
            Self::Escaped => "escaped",
            Self::Refused => "refused",
            Self::Inconclusive => "inconclusive",
            Self::Blocked => "blocked",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamProbe {
    pub id: String,
    pub verdict: RedteamOutcome,
}

/// One matrix cell (T2) or one scenario (T3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamCell {
    pub cell: String,
    pub verdict: RedteamOutcome,
    #[serde(default)]
    pub sink_hits: u32,
    #[serde(default)]
    pub probes: Vec<RedteamProbe>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamRunReport {
    pub schema: u32,
    pub tier: RedteamTier,
    /// The T3 harness lane; empty for T2, which covers every harness in one run.
    #[serde(default)]
    pub lane: String,
    pub run_id: String,
    pub version: String,
    pub verdict: RedteamOutcome,
    #[serde(default)]
    pub model: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    #[cfg_attr(test, ts(type = "number | null"))]
    pub started_at: Option<i64>,
    /// Unix seconds.
    #[cfg_attr(test, ts(type = "number"))]
    pub finished_at: i64,
    #[serde(default)]
    pub run_url: Option<String>,
    pub cells: Vec<RedteamCell>,
    #[serde(default)]
    pub reasons: Vec<String>,
}

/// An escaped cell, with the probe that escaped when one did. An empty `cell`
/// is a run-level escape no cell accounts for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamEscape {
    pub cell: String,
    pub probe: Option<String>,
}

impl RedteamRunReport {
    #[must_use]
    pub fn escapes(&self) -> Vec<RedteamEscape> {
        let mut out = Vec::new();
        for cell in &self.cells {
            let probes: Vec<&RedteamProbe> =
                cell.probes.iter().filter(|p| p.verdict == RedteamOutcome::Escaped).collect();
            if probes.is_empty() {
                if cell.verdict == RedteamOutcome::Escaped {
                    out.push(RedteamEscape { cell: cell.cell.clone(), probe: None });
                }
            } else {
                out.extend(
                    probes.into_iter().map(|p| RedteamEscape {
                        cell: cell.cell.clone(),
                        probe: Some(p.id.clone()),
                    }),
                );
            }
        }
        if out.is_empty() && self.verdict == RedteamOutcome::Escaped {
            out.push(RedteamEscape { cell: String::new(), probe: None });
        }
        out
    }

    /// # Errors
    ///
    /// The first violated bound, as a message for the caller.
    pub fn validate(&self) -> Result<(), String> {
        if self.schema != REDTEAM_REPORT_SCHEMA {
            return Err(format!("schema {} is not {REDTEAM_REPORT_SCHEMA}", self.schema));
        }
        if !is_safe_version(&self.version) {
            return Err("version must be 1-64 of [0-9A-Za-z.+-]".to_owned());
        }
        if self.run_id.is_empty() || self.run_id.len() > 64 {
            return Err("run_id must be 1-64 bytes".to_owned());
        }
        if self.cells.len() > REDTEAM_MAX_CELLS {
            return Err(format!("at most {REDTEAM_MAX_CELLS} cells"));
        }
        if let Some(url) = &self.run_url
            && !(url.starts_with("https://") && url.len() <= REDTEAM_MAX_TEXT)
        {
            return Err("run_url must be an https URL".to_owned());
        }
        let mut texts = std::iter::once(self.lane.as_str())
            .chain(self.model.as_deref())
            .chain(self.reasons.iter().map(String::as_str))
            .chain(self.cells.iter().flat_map(|c| {
                std::iter::once(c.cell.as_str())
                    .chain(c.reasons.iter().map(String::as_str))
                    .chain(c.probes.iter().map(|p| p.id.as_str()))
            }));
        if texts.any(|t| t.len() > REDTEAM_MAX_TEXT) {
            return Err(format!("text fields are at most {REDTEAM_MAX_TEXT} bytes"));
        }
        let too_many = self.reasons.len() > REDTEAM_MAX_REASONS
            || self.cells.iter().any(|c| {
                c.probes.len() > REDTEAM_MAX_PROBES || c.reasons.len() > REDTEAM_MAX_REASONS
            });
        if too_many {
            return Err(format!(
                "at most {REDTEAM_MAX_PROBES} probes and {REDTEAM_MAX_REASONS} reasons per entry"
            ));
        }
        Ok(())
    }
}

/// `[0-9A-Za-z.+-]`, 1 to 64 bytes.
#[must_use]
pub fn is_safe_version(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 64
        && v.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamRunAccepted {
    #[cfg_attr(test, ts(type = "number"))]
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamRun {
    #[cfg_attr(test, ts(type = "number"))]
    pub id: i64,
    /// Unix seconds.
    #[cfg_attr(test, ts(type = "number"))]
    pub received_at: i64,
    pub posted_by: Option<String>,
    pub report: RedteamRunReport,
    pub escapes: Vec<RedteamEscape>,
}

/// The latest T2 run and the latest T3 run per lane on one version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamVersion {
    pub version: String,
    pub t2: Option<RedteamRun>,
    pub t3: Vec<RedteamRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct RedteamOverview {
    pub current_version: String,
    /// Most recently reported first.
    pub versions: Vec<RedteamVersion>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> RedteamRunReport {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../redteam/ci/run-report.fixture.json");
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    fn cell(name: &str, verdict: RedteamOutcome, probes: &[(&str, RedteamOutcome)]) -> RedteamCell {
        RedteamCell {
            cell: name.to_owned(),
            verdict,
            sink_hits: 0,
            probes: probes
                .iter()
                .map(|(id, verdict)| RedteamProbe { id: (*id).to_owned(), verdict: *verdict })
                .collect(),
            reasons: Vec::new(),
        }
    }

    #[test]
    fn the_shared_fixture_parses_and_validates() {
        let report = fixture();
        assert_eq!(report.tier, RedteamTier::T2);
        assert_eq!(report.validate(), Ok(()));
        assert_eq!(
            report.escapes(),
            vec![RedteamEscape {
                cell: "codex/kube/transparent/root".to_owned(),
                probe: Some("EGR-04".to_owned())
            }]
        );
    }

    #[test]
    fn escapes_name_the_escaped_probe_or_the_cell_or_the_run() {
        use RedteamOutcome::{Blocked, Contained, Escaped};
        let mut report = fixture();
        report.cells = vec![
            cell("a", Contained, &[("EGR-01", Blocked)]),
            cell("b", Escaped, &[("EGR-01", Blocked)]),
        ];
        assert_eq!(report.escapes(), vec![RedteamEscape { cell: "b".to_owned(), probe: None }]);
        report.cells = vec![cell("a", Contained, &[("EGR-01", Blocked)])];
        report.verdict = Escaped;
        assert_eq!(report.escapes(), vec![RedteamEscape { cell: String::new(), probe: None }]);
        report.verdict = Contained;
        assert!(report.escapes().is_empty());
    }

    #[test]
    fn validation_refuses_out_of_bound_reports() {
        let mut bad = fixture();
        bad.schema = 2;
        assert!(bad.validate().is_err());
        let mut bad = fixture();
        bad.version = "../x".to_owned();
        assert!(bad.validate().is_err());
        let mut bad = fixture();
        bad.run_url = Some("http://example.com/run".to_owned());
        assert!(bad.validate().is_err());
        let mut bad = fixture();
        bad.cells = vec![cell("c", RedteamOutcome::Contained, &[]); REDTEAM_MAX_CELLS + 1];
        assert!(bad.validate().is_err());
        let mut bad = fixture();
        bad.cells[0].reasons = vec!["x".repeat(REDTEAM_MAX_TEXT + 1)];
        assert!(bad.validate().is_err());
    }
}
