//! The probe contract: each probe prints one line
//! `{"id":"EGR-01","verdict":"blocked|escaped|skipped","detail":"..."}`.

use serde::{Deserialize, Serialize};

/// `blocked` = bypass refused; `escaped` = bypass reached its target;
/// `skipped` = probe could not run in this cell, never counts as a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeVerdict {
    Blocked,
    Escaped,
    Skipped,
}

/// One parsed probe line.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProbeResult {
    pub id: String,
    pub verdict: ProbeVerdict,
    #[serde(default)]
    pub detail: String,
}

/// Parse probe output into results, keeping only well-formed contract lines.
///
/// Probe stdout interleaves human log lines with the JSON contract lines, so a
/// line that is not a complete `ProbeResult` object is noise and dropped. A
/// JSON object that parses but lacks `id`/`verdict` is not a probe line and is
/// likewise dropped, never guessed at.
#[must_use]
pub fn parse_probe_lines(output: &str) -> Vec<ProbeResult> {
    output
        .lines()
        .map(str::trim)
        .filter(|line| line.starts_with('{'))
        .filter_map(|line| serde_json::from_str::<ProbeResult>(line).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_contract_line_round_trips_through_json() {
        let line = r#"{"id":"EGR-01","verdict":"escaped","detail":"HTTP 200 from example.com"}"#;
        let parsed: ProbeResult = serde_json::from_str(line).unwrap();
        assert_eq!(parsed.id, "EGR-01");
        assert_eq!(parsed.verdict, ProbeVerdict::Escaped);
        assert_eq!(parsed.detail, "HTTP 200 from example.com");
    }

    #[test]
    fn the_detail_field_is_optional() {
        let parsed: ProbeResult =
            serde_json::from_str(r#"{"id":"PRIV-01","verdict":"blocked"}"#).unwrap();
        assert_eq!(parsed.verdict, ProbeVerdict::Blocked);
        assert!(parsed.detail.is_empty());
    }

    #[test]
    fn log_noise_and_malformed_lines_are_dropped_not_guessed() {
        let output = concat!(
            "probe: uid=1000 net_mode=transparent\n",
            r#"{"id":"EGR-01","verdict":"blocked","detail":"no response"}"#,
            "\n",
            "some stray log line\n",
            r#"{"id":"EGR-02","verdict":"escaped"}"#,
            "\n",
            r#"{"not":"a probe line"}"#,
            "\n",
            "{ this is not json }\n",
            r#"  {"id":"SEC-01","verdict":"skipped","detail":"no token mounted"}  "#,
            "\n",
        );
        let got = parse_probe_lines(output);
        assert_eq!(got.len(), 3);
        assert_eq!(got[0].id, "EGR-01");
        assert_eq!(got[1].verdict, ProbeVerdict::Escaped);
        assert_eq!(got[2].verdict, ProbeVerdict::Skipped);
    }

    #[test]
    fn an_unknown_verdict_value_is_not_a_probe_line() {
        assert!(parse_probe_lines(r#"{"id":"X","verdict":"reached"}"#).is_empty());
    }
}
