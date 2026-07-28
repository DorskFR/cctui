//! Typed client for the `opencode serve` HTTP API.
//!
//! Hand-rolled against the server's `OpenAPI` document (`GET /doc`, opencode
//! 1.18.7). Response types tolerate unknown fields so a schema-growing release
//! still deserializes.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Version the worker image bakes and the adapter is written against.
pub const OPENCODE_PINNED_VERSION: &str = "1.18.7";

/// Basic-auth username `opencode serve` expects when `OPENCODE_SERVER_PASSWORD`
/// is set (overridable upstream via `OPENCODE_SERVER_USERNAME`).
pub const SERVER_USERNAME: &str = "opencode";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub healthy: bool,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default, rename = "parentID", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
}

/// `POST /session` model reference — note the field names differ from the
/// prompt body's (`id` here, `modelID` there).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionModelRef {
    pub id: String,
    #[serde(rename = "providerID")]
    pub provider_id: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(rename = "parentID", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<SessionModelRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PromptModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PartInput {
    Text { text: String },
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct PromptRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<PromptModelRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub parts: Vec<PartInput>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenCache {
    #[serde(default, deserialize_with = "de_num_u64")]
    pub read: u64,
    #[serde(default, deserialize_with = "de_num_u64")]
    pub write: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tokens {
    #[serde(default, deserialize_with = "de_num_u64")]
    pub input: u64,
    #[serde(default, deserialize_with = "de_num_u64")]
    pub output: u64,
    #[serde(default, deserialize_with = "de_num_u64")]
    pub reasoning: u64,
    #[serde(default)]
    pub cache: TokenCache,
}

impl Tokens {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.input == 0 && self.output == 0 && self.cache.read == 0 && self.cache.write == 0
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageTime {
    #[serde(default)]
    pub created: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed: Option<i64>,
}

/// Union of `UserMessage` / `AssistantMessage`, discriminated by `role`.
/// Assistant-only fields stay optional so one struct covers both.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub time: MessageTime,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<Tokens>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<f64>,
    #[serde(default, rename = "modelID", skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,
    #[serde(default, rename = "providerID", skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartTime {
    #[serde(default)]
    pub start: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ToolState {
    Pending,
    Running {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Completed {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        output: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
    },
    Error {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        error: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Part {
    Text {
        id: String,
        #[serde(default, rename = "messageID", skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time: Option<PartTime>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        synthetic: Option<bool>,
    },
    Reasoning {
        id: String,
        #[serde(default, rename = "messageID", skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        #[serde(default)]
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        time: Option<PartTime>,
    },
    Tool {
        id: String,
        #[serde(default, rename = "messageID", skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        tool: String,
        #[serde(default, rename = "callID", skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        state: ToolState,
    },
    #[serde(other)]
    Other,
}

impl Part {
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::Text { id, .. } | Self::Reasoning { id, .. } | Self::Tool { id, .. } => id,
            Self::Other => "",
        }
    }

    #[must_use]
    pub fn message_id(&self) -> Option<&str> {
        match self {
            Self::Text { message_id, .. }
            | Self::Reasoning { message_id, .. }
            | Self::Tool { message_id, .. } => message_id.as_deref(),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageWithParts {
    pub info: MessageInfo,
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// Deserialize a JSON number that the opencode schema types as `number` but
/// always emits as a non-negative integer.
fn de_num_u64<'de, D>(de: D) -> Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(de)?;
    Ok(match v {
        serde_json::Value::Number(n) => n
            .as_u64()
            .or_else(|| n.as_f64().map(|f| if f > 0.0 { f.trunc() as u64 } else { 0 }))
            .unwrap_or(0),
        _ => 0,
    })
}

/// Per-request deadline. A freshly started `opencode serve` accepts the TCP
/// connection before its handlers are wired and never answers that first
/// request, so every non-streaming call must be bounded or the driver wedges.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Thin HTTP wrapper over one `opencode serve` instance.
pub struct OpenCodeClient {
    base: String,
    password: String,
    http: reqwest::Client,
}

impl OpenCodeClient {
    #[must_use]
    pub fn new(base: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            password: password.into(),
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default(),
        }
    }

    #[must_use]
    pub fn base(&self) -> &str {
        &self.base
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.stream_get(path).timeout(REQUEST_TIMEOUT)
    }

    /// Unbounded GET, for the event stream only.
    fn stream_get(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .get(format!("{}{path}", self.base))
            .basic_auth(SERVER_USERNAME, Some(&self.password))
    }

    fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.http
            .post(format!("{}{path}", self.base))
            .basic_auth(SERVER_USERNAME, Some(&self.password))
            .timeout(REQUEST_TIMEOUT)
    }

    pub async fn health(&self) -> Result<Health> {
        let resp = self.get("/global/health").send().await?.error_for_status()?;
        Ok(resp.json().await?)
    }

    pub async fn create_session(&self, body: &CreateSession) -> Result<SessionInfo> {
        let resp = self
            .post("/session")
            .json(body)
            .send()
            .await
            .context("POST /session")?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// Fire-and-forget prompt: the turn is observed through the event stream
    /// rather than a blocking response.
    pub async fn prompt_async(&self, session_id: &str, body: &PromptRequest) -> Result<()> {
        self.post(&format!("/session/{session_id}/prompt_async"))
            .json(body)
            .send()
            .await
            .context("POST prompt_async")?
            .error_for_status()?;
        Ok(())
    }

    pub async fn messages(&self, session_id: &str) -> Result<Vec<MessageWithParts>> {
        let resp = self.get(&format!("/session/{session_id}/message")).send().await?;
        Ok(resp.error_for_status()?.json().await?)
    }

    pub async fn abort(&self, session_id: &str) -> Result<bool> {
        let resp = self.post(&format!("/session/{session_id}/abort")).send().await?;
        Ok(resp.error_for_status()?.json().await?)
    }

    pub async fn fork(&self, session_id: &str) -> Result<SessionInfo> {
        let resp = self
            .post(&format!("/session/{session_id}/fork"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .context("POST fork")?
            .error_for_status()?;
        Ok(resp.json().await?)
    }

    /// `response` is one of `once` / `always` / `reject`.
    pub async fn respond_permission(
        &self,
        session_id: &str,
        permission_id: &str,
        response: &str,
    ) -> Result<()> {
        self.post(&format!("/session/{session_id}/permissions/{permission_id}"))
            .json(&serde_json::json!({ "response": response }))
            .send()
            .await
            .context("POST permission response")?
            .error_for_status()?;
        Ok(())
    }

    /// Open the SSE event stream. The caller decodes frames with
    /// [`super::events::SseDecoder`].
    pub async fn events(&self) -> Result<reqwest::Response> {
        Ok(self
            .stream_get("/event")
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .context("GET /event")?
            .error_for_status()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_info_round_trips() {
        let raw = serde_json::json!({
            "id": "ses_058c76895ffeEBPdyw7eco1u14",
            "slug": "probe",
            "projectID": "prj_1",
            "directory": "/repo",
            "title": "probe",
            "cost": 0,
            "tokens": { "input": 0, "output": 0, "reasoning": 0, "cache": { "read": 0, "write": 0 } }
        });
        let s: SessionInfo = serde_json::from_value(raw).unwrap();
        assert_eq!(s.id, "ses_058c76895ffeEBPdyw7eco1u14");
        assert_eq!(s.directory.as_deref(), Some("/repo"));
        assert_eq!(s.parent_id, None);
    }

    #[test]
    fn assistant_message_carries_tokens_and_cost() {
        let raw = serde_json::json!({
            "id": "msg_1",
            "sessionID": "ses_1",
            "role": "assistant",
            "parentID": "msg_0",
            "mode": "cctui-reviewer",
            "agent": "cctui-reviewer",
            "path": { "cwd": "/repo", "root": "/repo" },
            "modelID": "accounts/fireworks/models/kimi-k3",
            "providerID": "fireworks-ai",
            "cost": 0.0123,
            "tokens": { "input": 120, "output": 42, "reasoning": 0, "cache": { "read": 7, "write": 3 } },
            "time": { "created": 1_785_216_933_333i64, "completed": 1_785_216_940_000i64 }
        });
        let m: MessageInfo = serde_json::from_value(raw).unwrap();
        assert_eq!(m.role, "assistant");
        let t = m.tokens.unwrap();
        assert_eq!((t.input, t.output, t.cache.read, t.cache.write), (120, 42, 7, 3));
        assert_eq!(m.time.completed, Some(1_785_216_940_000));
        assert_eq!(m.model_id.as_deref(), Some("accounts/fireworks/models/kimi-k3"));
    }

    #[test]
    fn user_message_has_no_tokens() {
        let raw = serde_json::json!({
            "id": "msg_0",
            "sessionID": "ses_1",
            "role": "user",
            "time": { "created": 1_785_216_931_842i64 }
        });
        let m: MessageInfo = serde_json::from_value(raw).unwrap();
        assert!(m.tokens.is_none());
        assert_eq!(m.time.completed, None);
    }

    #[test]
    fn float_token_counts_truncate() {
        let t: Tokens =
            serde_json::from_value(serde_json::json!({ "input": 10.0, "output": 3.9 })).unwrap();
        assert_eq!((t.input, t.output), (10, 3));
        assert!(Tokens::default().is_empty());
    }

    #[test]
    fn text_part_round_trips() {
        let raw = serde_json::json!({
            "type": "text",
            "id": "prt_1",
            "sessionID": "ses_1",
            "messageID": "msg_1",
            "text": "hello",
            "time": { "start": 1, "end": 2 }
        });
        let p: Part = serde_json::from_value(raw).unwrap();
        match &p {
            Part::Text { text, time, .. } => {
                assert_eq!(text, "hello");
                assert_eq!(time.unwrap().end, Some(2));
            }
            other => panic!("expected text part, got {other:?}"),
        }
        assert_eq!(p.id(), "prt_1");
        assert_eq!(p.message_id(), Some("msg_1"));
    }

    #[test]
    fn tool_part_states_round_trip() {
        let running = serde_json::json!({
            "type": "tool", "id": "prt_2", "sessionID": "ses_1", "messageID": "msg_1",
            "callID": "call_1", "tool": "read",
            "state": { "status": "running", "input": { "filePath": "a.rs" }, "time": { "start": 1 } }
        });
        assert!(matches!(
            serde_json::from_value::<Part>(running).unwrap(),
            Part::Tool { state: ToolState::Running { .. }, .. }
        ));

        let completed = serde_json::json!({
            "type": "tool", "id": "prt_2", "sessionID": "ses_1", "messageID": "msg_1",
            "callID": "call_1", "tool": "read",
            "state": { "status": "completed", "input": { "filePath": "a.rs" }, "output": "fn main() {}",
                       "title": "a.rs", "metadata": {}, "time": { "start": 1, "end": 2 } }
        });
        match serde_json::from_value::<Part>(completed).unwrap() {
            Part::Tool { tool, state: ToolState::Completed { output, .. }, .. } => {
                assert_eq!(tool, "read");
                assert_eq!(output, "fn main() {}");
            }
            other => panic!("expected completed tool part, got {other:?}"),
        }

        let errored = serde_json::json!({
            "type": "tool", "id": "prt_3", "sessionID": "ses_1", "messageID": "msg_1",
            "callID": "call_2", "tool": "bash",
            "state": { "status": "error", "input": {}, "error": "denied", "time": { "start": 1, "end": 2 } }
        });
        assert!(matches!(
            serde_json::from_value::<Part>(errored).unwrap(),
            Part::Tool { state: ToolState::Error { .. }, .. }
        ));
    }

    #[test]
    fn unknown_part_kind_degrades_to_other() {
        let raw = serde_json::json!({ "type": "step-start", "id": "prt_9" });
        assert_eq!(serde_json::from_value::<Part>(raw).unwrap(), Part::Other);
    }

    #[test]
    fn prompt_request_serializes_to_the_documented_shape() {
        let body = PromptRequest {
            model: Some(PromptModelRef {
                provider_id: "fireworks-ai".to_owned(),
                model_id: "accounts/fireworks/models/kimi-k3".to_owned(),
            }),
            agent: Some("cctui-reviewer".to_owned()),
            parts: vec![PartInput::Text { text: "hi".to_owned() }],
        };
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["model"]["providerID"], "fireworks-ai");
        assert_eq!(v["model"]["modelID"], "accounts/fireworks/models/kimi-k3");
        assert_eq!(v["agent"], "cctui-reviewer");
        assert_eq!(v["parts"][0]["type"], "text");
        assert_eq!(v["parts"][0]["text"], "hi");
    }

    #[test]
    fn create_session_omits_unset_fields() {
        let v = serde_json::to_value(CreateSession {
            title: Some("PR review".to_owned()),
            agent: Some("cctui-reviewer".to_owned()),
            ..CreateSession::default()
        })
        .unwrap();
        assert_eq!(v["title"], "PR review");
        assert!(v.get("parentID").is_none());
        assert!(v.get("model").is_none());
    }

    #[test]
    fn health_round_trips() {
        let h: Health = serde_json::from_str(r#"{"healthy":true,"version":"1.18.7"}"#).unwrap();
        assert!(h.healthy);
        assert_eq!(h.version, OPENCODE_PINNED_VERSION);
    }
}
