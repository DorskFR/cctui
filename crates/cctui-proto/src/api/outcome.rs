//! Session outcomes: the structured end of a session's work.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(test)]
use ts_rs::TS;
use uuid::Uuid;

pub const OUTCOME_HEADLINE_MAX: usize = 200;
pub const OUTCOME_BODY_MAX: usize = 16_384;
pub const OUTCOME_EVIDENCE_MAX: usize = 20;
pub const OUTCOME_EVIDENCE_ITEM_MAX: usize = 2048;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OutcomeStatus {
    Result,
    NeedsInput,
    Failed,
    EndedWithoutOutcome,
}

impl OutcomeStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Result => "result",
            Self::NeedsInput => "needs_input",
            Self::Failed => "failed",
            Self::EndedWithoutOutcome => "ended_without_outcome",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "result" => Some(Self::Result),
            "needs_input" => Some(Self::NeedsInput),
            "failed" => Some(Self::Failed),
            "ended_without_outcome" => Some(Self::EndedWithoutOutcome),
            _ => None,
        }
    }

    /// The `status` the dispatch callback has always carried, so a flow that
    /// switches on `success` / `failed` keeps working.
    #[must_use]
    pub const fn callback_status(self) -> &'static str {
        match self {
            Self::Result => "success",
            Self::NeedsInput => "needs_input",
            Self::Failed | Self::EndedWithoutOutcome => "failed",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OutcomeSource {
    #[default]
    Tool,
    ResultFile,
    Text,
    Inferred,
}

impl OutcomeSource {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tool => "tool",
            Self::ResultFile => "result_file",
            Self::Text => "text",
            Self::Inferred => "inferred",
        }
    }

    #[must_use]
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "tool" => Some(Self::Tool),
            "result_file" => Some(Self::ResultFile),
            "text" => Some(Self::Text),
            "inferred" => Some(Self::Inferred),
            _ => None,
        }
    }

    /// Only declared outcomes reach the dispatch callback.
    #[must_use]
    pub const fn is_declared(self) -> bool {
        !matches!(self, Self::Inferred)
    }
}

/// Body for `POST /api/v1/daemon/sessions/{id}/outcome`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct DeclareOutcomeRequest {
    pub status: OutcomeStatus,
    pub headline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    #[serde(default)]
    pub source: OutcomeSource,
}

impl DeclareOutcomeRequest {
    /// # Errors
    /// An empty headline, or `ended_without_outcome` from anything but the
    /// server's inference.
    pub fn normalized(self) -> Result<Self, String> {
        let headline: String = self
            .headline
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or_default()
            .chars()
            .take(OUTCOME_HEADLINE_MAX)
            .collect();
        if headline.is_empty() {
            return Err("headline is required and was empty".to_owned());
        }
        let inferred = self.source == OutcomeSource::Inferred;
        if self.status == OutcomeStatus::EndedWithoutOutcome && !inferred {
            return Err("status must be \"result\", \"needs_input\" or \"failed\"".to_owned());
        }
        let body = self
            .body
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .map(|b| b.chars().take(OUTCOME_BODY_MAX).collect());
        let mut evidence: Vec<String> = Vec::new();
        for item in &self.evidence {
            let item: String = item.trim().chars().take(OUTCOME_EVIDENCE_ITEM_MAX).collect();
            let room = evidence.len() < OUTCOME_EVIDENCE_MAX;
            if room && !item.is_empty() && !evidence.contains(&item) {
                evidence.push(item);
            }
        }
        Ok(Self { status: self.status, headline, body, evidence, source: self.source })
    }

    /// The outcome a worker's `RESULT_FILE` verdict stands for.
    ///
    /// `status` `success` is a result and `needs_input` / `needs_human` asks
    /// for input; any other value is a failure. The first line of `message` is the headline
    /// and the rest its body; a failure's `error` is its headline.
    #[must_use]
    pub fn from_result_file(verdict: &Value) -> Self {
        let text = |key: &str| {
            verdict.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
        };
        let status = match text("status") {
            Some("success" | "result") => OutcomeStatus::Result,
            Some("needs_input" | "needs-input" | "needs_human") => OutcomeStatus::NeedsInput,
            _ => OutcomeStatus::Failed,
        };
        let (first, rest) = text("message").map_or((None, None), split_message);
        let (headline, body) = match (status, text("error")) {
            (OutcomeStatus::Failed, Some(error)) => {
                (error.to_owned(), text("message").map(str::to_owned))
            }
            (OutcomeStatus::Failed, None) => {
                (first.unwrap_or_else(|| "the worker reported a failure".to_owned()), rest)
            }
            _ => (first.unwrap_or_else(|| "the worker wrote its result".to_owned()), rest),
        };
        let evidence = verdict
            .get("evidence")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(evidence_line).collect())
            .unwrap_or_default();
        Self { status, headline, body, evidence, source: OutcomeSource::ResultFile }
    }
}

/// A string entry as written; a structured `{ kind, summary, … }` entry by its
/// summary, else as JSON.
fn evidence_line(item: &Value) -> Option<String> {
    match item {
        Value::String(s) => Some(s.clone()),
        Value::Object(o) => {
            let summary = o.get("summary").and_then(Value::as_str);
            Some(summary.map_or_else(|| item.to_string(), str::to_owned))
        }
        _ => None,
    }
}

fn split_message(message: &str) -> (Option<String>, Option<String>) {
    let message = message.trim();
    let (head, rest) = message.split_once('\n').unwrap_or((message, ""));
    let head = head.trim();
    let rest = rest.trim();
    ((!head.is_empty()).then(|| head.to_owned()), (!rest.is_empty()).then(|| rest.to_owned()))
}

/// The outcome a `result:` / `needs input:` / `failed:` line in a turn's final
/// text declares. The last such line outside a code fence wins; the prefix is
/// case-insensitive and may be wrapped in `**`.
#[must_use]
pub fn parse_text_outcome(text: &str) -> Option<DeclareOutcomeRequest> {
    const PREFIXES: [(&str, OutcomeStatus); 4] = [
        ("result:", OutcomeStatus::Result),
        ("needs input:", OutcomeStatus::NeedsInput),
        ("needs_input:", OutcomeStatus::NeedsInput),
        ("failed:", OutcomeStatus::Failed),
    ];
    let mut fenced = false;
    let mut found = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if fenced {
            continue;
        }
        let bare = line.trim_start_matches('*').trim_start();
        let lower = bare.to_ascii_lowercase();
        for (prefix, status) in PREFIXES {
            if lower.starts_with(prefix)
                && let Some(rest) = bare.get(prefix.len()..)
            {
                let headline = rest.trim().trim_start_matches('*').trim_end_matches('*').trim();
                if !headline.is_empty() {
                    found = Some((status, headline.to_owned()));
                }
                break;
            }
        }
    }
    found.map(|(status, headline)| DeclareOutcomeRequest {
        status,
        headline,
        body: None,
        evidence: Vec::new(),
        source: OutcomeSource::Text,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct SessionOutcome {
    pub id: Uuid,
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_seq: Option<i64>,
    pub status: OutcomeStatus,
    pub headline: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<String>,
    pub source: OutcomeSource,
    pub created_at: DateTime<Utc>,
}

impl From<&SessionOutcome> for DeclareOutcomeRequest {
    fn from(o: &SessionOutcome) -> Self {
        Self {
            status: o.status,
            headline: o.headline.clone(),
            body: o.body.clone(),
            evidence: o.evidence.clone(),
            source: o.source,
        }
    }
}

/// A rejection is a tool-level result the model reads, not an HTTP error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(test, derive(TS), ts(export))]
pub struct DeclareOutcomeResult {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<SessionOutcome>,
    #[serde(default)]
    pub callback_queued: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn text(raw: &str) -> Option<(OutcomeStatus, String)> {
        parse_text_outcome(raw).map(|o| (o.status, o.headline))
    }

    #[test]
    fn each_text_prefix_maps_to_its_status() {
        assert_eq!(text("result: PR opened"), Some((OutcomeStatus::Result, "PR opened".into())));
        assert_eq!(
            text("Needs input: which branch?"),
            Some((OutcomeStatus::NeedsInput, "which branch?".into()))
        );
        assert_eq!(text("FAILED: no repo"), Some((OutcomeStatus::Failed, "no repo".into())));
        assert_eq!(
            text("**result:** shipped 0.3"),
            Some((OutcomeStatus::Result, "shipped 0.3".into()))
        );
    }

    #[test]
    fn the_last_line_outside_a_fence_wins_and_prose_is_ignored() {
        let raw = "Done.\nresult: first\n\n```\nfailed: example inside a fence\n```\n\
                   needs input: the token";
        assert_eq!(text(raw), Some((OutcomeStatus::NeedsInput, "the token".into())));
        assert_eq!(text("The result: was fine, mid-sentence"), None);
        assert_eq!(text("no declaration here"), None);
        assert_eq!(text("result:   "), None);
        assert_eq!(parse_text_outcome("result: x").unwrap().source, OutcomeSource::Text);
    }

    #[test]
    fn normalizing_keeps_one_capped_line_and_distinct_evidence() {
        let req = DeclareOutcomeRequest {
            status: OutcomeStatus::Result,
            headline: format!("\n  {}\nsecond line", "h".repeat(OUTCOME_HEADLINE_MAX + 5)),
            body: Some("   ".into()),
            evidence: vec![" a ".into(), "a".into(), String::new(), "b".into()],
            source: OutcomeSource::Tool,
        }
        .normalized()
        .unwrap();
        assert_eq!(req.headline.chars().count(), OUTCOME_HEADLINE_MAX);
        assert!(req.body.is_none());
        assert_eq!(req.evidence, ["a", "b"]);
    }

    #[test]
    fn normalizing_refuses_an_empty_headline_and_a_declared_ended_without_outcome() {
        let mut req = DeclareOutcomeRequest {
            status: OutcomeStatus::Result,
            headline: "  \n ".into(),
            body: None,
            evidence: Vec::new(),
            source: OutcomeSource::Tool,
        };
        assert!(req.clone().normalized().is_err());
        req.headline = "x".into();
        req.status = OutcomeStatus::EndedWithoutOutcome;
        assert!(req.clone().normalized().is_err());
        req.source = OutcomeSource::Inferred;
        assert!(req.normalized().is_ok());
    }

    #[test]
    fn a_result_file_verdict_converts_to_the_outcome_a_tool_would_declare() {
        let file = DeclareOutcomeRequest::from_result_file(&json!({
            "task_id": "T-1", "flow": "codex-dispatch", "status": "success", "error": null,
            "message": "Opened PR 12\n\nAll checks pass.",
            "evidence": ["https://example.test/pr/12"],
        }));
        assert_eq!(file.status, OutcomeStatus::Result);
        assert_eq!(file.headline, "Opened PR 12");
        assert_eq!(file.body.as_deref(), Some("All checks pass."));
        assert_eq!(file.evidence, ["https://example.test/pr/12"]);
        assert_eq!(file.source, OutcomeSource::ResultFile);

        let failed = DeclareOutcomeRequest::from_result_file(&json!({
            "status": "failed", "error": "boom", "message": "partial work",
        }));
        assert_eq!(failed.status, OutcomeStatus::Failed);
        assert_eq!(failed.headline, "boom");
        assert_eq!(failed.body.as_deref(), Some("partial work"));

        let human = DeclareOutcomeRequest::from_result_file(&json!({
            "status": "needs_human", "message": "approve the deploy",
            "evidence": [{ "kind": "test-run", "summary": "14 tests pass" }, 3],
        }));
        assert_eq!(human.status, OutcomeStatus::NeedsInput);
        assert_eq!(human.evidence, ["14 tests pass"]);

        let bare = DeclareOutcomeRequest::from_result_file(&json!({ "status": "weird" }));
        assert_eq!(bare.status, OutcomeStatus::Failed);
        assert!(bare.normalized().is_ok(), "a verdict with no text still has a headline");
    }

    #[test]
    fn statuses_and_sources_round_trip_their_wire_names() {
        for s in [
            OutcomeStatus::Result,
            OutcomeStatus::NeedsInput,
            OutcomeStatus::Failed,
            OutcomeStatus::EndedWithoutOutcome,
        ] {
            assert_eq!(OutcomeStatus::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        for s in [
            OutcomeSource::Tool,
            OutcomeSource::ResultFile,
            OutcomeSource::Text,
            OutcomeSource::Inferred,
        ] {
            assert_eq!(OutcomeSource::parse(s.as_str()), Some(s));
            assert_eq!(serde_json::to_value(s).unwrap(), json!(s.as_str()));
        }
        assert_eq!(OutcomeStatus::Result.callback_status(), "success");
        assert!(!OutcomeSource::Inferred.is_declared());
    }
}
