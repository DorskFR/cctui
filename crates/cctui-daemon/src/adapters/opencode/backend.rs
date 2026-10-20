//! What the session driver asks of `opencode serve`, routed to the API
//! generation the server answered the detection probe with.

use anyhow::Result;
use cctui_proto::adapter::AdapterEvent;
use serde_json::Value;

use super::client::{
    CreateSession, OPENCODE_PINNED_VERSION, OpenCodeClient, PartInput, PromptModelRef,
    PromptRequest, SessionModelRef,
};
use super::config::ModelRef;
use super::events::OcEvent;
use super::normalize::Kind;
use super::v2;

/// A v1 server answers unknown paths with its web app, so a slow `/api/info`
/// must not hold up the `/global/health` fallback.
const INFO_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Generation {
    #[default]
    V1,
    V2,
}

impl Generation {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::V1 => "v1",
            Self::V2 => "v2",
        }
    }

    #[must_use]
    pub const fn pinned(self) -> &'static str {
        match self {
            Self::V1 => OPENCODE_PINNED_VERSION,
            Self::V2 => v2::client::PINNED_VERSION,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
    pub generation: Generation,
    pub version: String,
}

#[derive(Debug, Clone, Copy)]
pub struct NewSession<'a> {
    pub title: Option<&'a str>,
    pub agent: Option<&'a str>,
    pub model: Option<&'a ModelRef>,
    pub directory: &'a str,
}

#[derive(Debug, Clone)]
pub enum Incoming {
    V1(OcEvent),
    V2(v2::events::Event),
}

impl Incoming {
    #[must_use]
    pub fn session_id(&self) -> Option<&str> {
        match self {
            Self::V1(evt) => evt.session_id(),
            Self::V2(evt) => evt.session_id(),
        }
    }
}

/// What a bus event means to the driver, whatever the generation.
#[derive(Debug, Clone)]
pub enum Signal {
    Rows(Vec<(Kind, Value)>),
    /// Streamed assistant text for a watched session; the final row wins.
    TextDelta {
        item_id: String,
        delta: String,
    },
    AssistantActive,
    Usage(Box<AdapterEvent>),
    Tempo {
        tempo: String,
        detail: Option<String>,
    },
    TurnEnded,
    Failed(String),
    Deleted,
    PermissionAsked {
        request_id: String,
        tool: String,
        input: Value,
    },
    PermissionReplied {
        request_id: String,
    },
}

fn session_model(m: &ModelRef) -> SessionModelRef {
    SessionModelRef { id: m.model_id.clone(), provider_id: m.provider_id.clone() }
}

impl OpenCodeClient {
    /// `/api/info` answering JSON means v2. Both generations answer paths they
    /// do not serve with their web app and a 200, so only a JSON body counts.
    pub async fn detect(&self) -> Result<ServerInfo> {
        if let Ok(Ok(Some(info))) = tokio::time::timeout(INFO_PROBE_TIMEOUT, self.v2_info()).await {
            return Ok(ServerInfo { generation: Generation::V2, version: info.version });
        }
        let health = self.health().await?;
        anyhow::ensure!(health.healthy, "unhealthy (version {})", health.version);
        Ok(ServerInfo { generation: Generation::V1, version: health.version })
    }

    pub async fn open_session(&self, req: &NewSession<'_>) -> Result<String> {
        let title = req.title.map(str::to_owned);
        let agent = req.agent.map(str::to_owned);
        let model = req.model.map(session_model);
        match self.generation() {
            Generation::V1 => {
                let body = CreateSession { title, agent, parent_id: None, model };
                Ok(self.create_session(&body).await?.id)
            }
            Generation::V2 => {
                let location = std::path::Path::new(req.directory)
                    .is_absolute()
                    .then(|| v2::client::Location { directory: req.directory.to_owned() });
                let body = v2::client::CreateSession { title, agent, model, location };
                Ok(self.v2_create_session(&body).await?.id)
            }
        }
    }

    /// v2 takes no model or agent per prompt: the session already carries them.
    pub async fn send_prompt(
        &self,
        session_id: &str,
        text: &str,
        model: Option<&ModelRef>,
        agent: Option<String>,
    ) -> Result<()> {
        match self.generation() {
            Generation::V1 => {
                let body = PromptRequest {
                    model: model.map(|m| PromptModelRef {
                        provider_id: m.provider_id.clone(),
                        model_id: m.model_id.clone(),
                    }),
                    agent,
                    parts: vec![PartInput::Text { text: text.to_owned() }],
                };
                self.prompt_async(session_id, &body).await
            }
            Generation::V2 => {
                let body = v2::client::PromptRequest { text: text.to_owned(), delivery: None };
                self.v2_prompt(session_id, &body).await
            }
        }
    }

    pub async fn interrupt(&self, session_id: &str) -> Result<()> {
        match self.generation() {
            Generation::V1 => {
                self.abort(session_id).await?;
                Ok(())
            }
            Generation::V2 => self.v2_interrupt(session_id).await,
        }
    }

    /// On v2 the fork's model and agent are set through their own endpoints;
    /// a failure there leaves the fork on what it inherited.
    pub async fn fork_session(
        &self,
        parent: &str,
        model: Option<&ModelRef>,
        agent: Option<&str>,
    ) -> Result<String> {
        match self.generation() {
            Generation::V1 => Ok(self.fork(parent).await?.id),
            Generation::V2 => {
                let child = self.v2_fork(parent).await?;
                if let Some(m) = model
                    && let Err(err) = self.v2_set_model(&child.id, &session_model(m)).await
                {
                    tracing::warn!(%err, session = %child.id, "opencode: fork model not set");
                }
                if let Some(agent) = agent
                    && let Err(err) = self.v2_set_agent(&child.id, agent).await
                {
                    tracing::warn!(%err, session = %child.id, "opencode: fork agent not set");
                }
                Ok(child.id)
            }
        }
    }

    pub async fn reply_permission(
        &self,
        session_id: &str,
        request_id: &str,
        allow: bool,
    ) -> Result<()> {
        match self.generation() {
            Generation::V1 => {
                let response = if allow { "once" } else { "reject" };
                self.respond_permission(session_id, request_id, response).await
            }
            Generation::V2 => {
                let decision =
                    if allow { v2::client::Decision::Once } else { v2::client::Decision::Reject };
                self.v2_reply_permission(session_id, request_id, decision).await
            }
        }
    }

    pub async fn session_exists(&self, session_id: &str) -> Result<()> {
        match self.generation() {
            Generation::V1 => {
                self.messages(session_id).await?;
            }
            Generation::V2 => {
                self.v2_session(session_id).await?;
            }
        }
        Ok(())
    }

    pub async fn event_stream(&self) -> Result<reqwest::Response> {
        match self.generation() {
            Generation::V1 => self.events().await,
            Generation::V2 => self.v2_events().await,
        }
    }

    pub fn decode_event(&self, data: &str) -> serde_json::Result<Incoming> {
        match self.generation() {
            Generation::V1 => serde_json::from_str::<OcEvent>(data).map(Incoming::V1),
            Generation::V2 => v2::events::Event::decode(data).map(Incoming::V2),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::opencode::fake_serve::{self, Reply};

    const V2_INFO: &str =
        r#"{"version":"2.0.27","pid":7,"urls":["http://127.0.0.1:4096"],"paths":{"tmp":"/tmp"}}"#;
    const V1_HEALTH: &str = r#"{"healthy":true,"version":"1.19.3"}"#;
    const V2_SESSION: &str = r#"{"data":{"id":"ses_child","projectID":"p","cost":0,
        "tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},
        "time":{"created":1,"updated":1},"location":{"directory":"/work"}}}"#;

    fn v2_server(_method: &str, path: &str) -> Reply {
        match path {
            "/api/info" => Reply::json(V2_INFO),
            _ => Reply::html(),
        }
    }

    fn v1_html_server(_method: &str, path: &str) -> Reply {
        match path {
            "/global/health" => Reply::json(V1_HEALTH),
            _ => Reply::html(),
        }
    }

    fn v1_404_server(_method: &str, path: &str) -> Reply {
        match path {
            "/global/health" => Reply::json(V1_HEALTH),
            _ => Reply::status(404),
        }
    }

    fn v1_unhealthy_server(_method: &str, path: &str) -> Reply {
        match path {
            "/global/health" => Reply::json(r#"{"healthy":false,"version":"1.19.3"}"#),
            _ => Reply::html(),
        }
    }

    fn paths(seen: &fake_serve::Log) -> Vec<String> {
        seen.lock().unwrap().iter().map(|s| format!("{} {}", s.method, s.path)).collect()
    }

    #[tokio::test]
    async fn a_json_api_info_is_a_v2_server() {
        let (base, seen) = fake_serve::serve(v2_server).await;
        let info = OpenCodeClient::new(base, "pw").detect().await.unwrap();
        assert_eq!(info, ServerInfo { generation: Generation::V2, version: "2.0.27".to_owned() });
        assert_eq!(paths(&seen), vec!["GET /api/info"], "v2 needs no v1 fallback");
    }

    #[tokio::test]
    async fn an_html_api_info_falls_back_to_the_v1_health_check() {
        let (base, seen) = fake_serve::serve(v1_html_server).await;
        let info = OpenCodeClient::new(base, "pw").detect().await.unwrap();
        assert_eq!(info, ServerInfo { generation: Generation::V1, version: "1.19.3".to_owned() });
        assert_eq!(paths(&seen), vec!["GET /api/info", "GET /global/health"]);
    }

    #[tokio::test]
    async fn a_missing_api_info_falls_back_to_the_v1_health_check() {
        let (base, _seen) = fake_serve::serve(v1_404_server).await;
        let info = OpenCodeClient::new(base, "pw").detect().await.unwrap();
        assert_eq!(info.generation, Generation::V1);
    }

    #[tokio::test]
    async fn an_unhealthy_v1_server_is_not_ready() {
        let (base, _seen) = fake_serve::serve(v1_unhealthy_server).await;
        let err = OpenCodeClient::new(base, "pw").detect().await.unwrap_err();
        assert!(err.to_string().contains("unhealthy (version 1.19.3)"), "{err}");
    }

    /// v2 answers v1 paths with its web app: an HTML health page is no answer.
    #[tokio::test]
    async fn a_server_answering_html_everywhere_is_not_ready() {
        let (base, _seen) = fake_serve::serve(|_, _| Reply::html()).await;
        assert!(OpenCodeClient::new(base, "pw").detect().await.is_err());
    }

    #[test]
    fn each_generation_has_its_own_pin() {
        assert_eq!(Generation::V1.pinned(), OPENCODE_PINNED_VERSION);
        assert_eq!(Generation::V2.pinned(), v2::client::PINNED_VERSION);
        assert_eq!((Generation::V1.label(), Generation::V2.label()), ("v1", "v2"));
        assert_eq!(OpenCodeClient::new("http://x", "pw").generation(), Generation::V1);
    }

    fn any_session(method: &str, path: &str) -> Reply {
        match (method, path) {
            ("POST", "/session/ses_1/abort") => Reply::json("true"),
            ("POST", "/api/session/ses_1/interrupt") => Reply::json(r#"{"interrupted":true}"#),
            ("POST", "/api/session/ses_1/fork" | "/api/session")
            | ("GET", "/api/session/ses_1") => Reply::json(V2_SESSION),
            ("POST", p)
                if p.ends_with("/model") || p.ends_with("/agent") || p.ends_with("/reply") =>
            {
                Reply::status(204)
            }
            ("POST", "/api/session/ses_1/prompt") => Reply::json(r#"{"data":{"id":"msg_1"}}"#),
            _ => Reply::status(404),
        }
    }

    async fn client_for(generation: Generation) -> (OpenCodeClient, fake_serve::Log) {
        let (base, seen) = fake_serve::serve(any_session).await;
        let mut client = OpenCodeClient::new(base, "pw");
        client.set_generation(generation);
        (client, seen)
    }

    #[tokio::test]
    async fn an_interrupt_hits_each_generations_own_endpoint() {
        let (v1, seen_v1) = client_for(Generation::V1).await;
        v1.interrupt("ses_1").await.unwrap();
        assert_eq!(paths(&seen_v1), vec!["POST /session/ses_1/abort"]);

        let (v2, seen_v2) = client_for(Generation::V2).await;
        v2.interrupt("ses_1").await.unwrap();
        assert_eq!(paths(&seen_v2), vec!["POST /api/session/ses_1/interrupt"]);
    }

    #[tokio::test]
    async fn a_v2_session_opens_with_its_model_agent_and_directory() {
        let (client, seen) = client_for(Generation::V2).await;
        let model = ModelRef::parse("fireworks-ai/accounts/fireworks/models/kimi-k3").unwrap();
        let id = client
            .open_session(&NewSession {
                title: Some("probe"),
                agent: Some("cctui-reviewer"),
                model: Some(&model),
                directory: "/work",
            })
            .await
            .unwrap();
        assert_eq!(id, "ses_child");
        let body: Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
        assert_eq!(body["title"], "probe");
        assert_eq!(body["agent"], "cctui-reviewer");
        assert_eq!(body["model"]["id"], "accounts/fireworks/models/kimi-k3");
        assert_eq!(body["model"]["providerID"], "fireworks-ai");
        assert_eq!(body["location"]["directory"], "/work");
        assert!(body.get("parentID").is_none());
    }

    #[tokio::test]
    async fn a_relative_directory_is_not_sent_as_a_v2_location() {
        let (client, seen) = client_for(Generation::V2).await;
        let req = NewSession { title: None, agent: None, model: None, directory: "." };
        client.open_session(&req).await.unwrap();
        let body: Value = serde_json::from_str(&seen.lock().unwrap()[0].body).unwrap();
        assert_eq!(body, serde_json::json!({}));
    }

    #[tokio::test]
    async fn a_v2_prompt_carries_only_its_text() {
        let (client, seen) = client_for(Generation::V2).await;
        let model = ModelRef::parse("kimi-k3").unwrap();
        client
            .send_prompt("ses_1", "hi", Some(&model), Some("cctui-builder".to_owned()))
            .await
            .unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0].path, "/api/session/ses_1/prompt");
        assert_eq!(
            serde_json::from_str::<Value>(&seen[0].body).unwrap(),
            serde_json::json!({ "text": "hi" })
        );
    }

    #[tokio::test]
    async fn a_v2_fork_sets_the_childs_model_and_agent() {
        let (client, seen) = client_for(Generation::V2).await;
        let model = ModelRef::parse("kimi-k3").unwrap();
        let child =
            client.fork_session("ses_1", Some(&model), Some("cctui-builder")).await.unwrap();
        assert_eq!(child, "ses_child");
        let seen = seen.lock().unwrap().clone();
        let got: Vec<(&str, &str)> =
            seen.iter().map(|s| (s.path.as_str(), s.body.as_str())).collect();
        assert_eq!(got[0].0, "/api/session/ses_1/fork");
        assert_eq!(got[1].0, "/api/session/ses_child/model");
        assert_eq!(
            serde_json::from_str::<Value>(got[1].1).unwrap()["model"],
            serde_json::json!({ "id": "kimi-k3", "providerID": "fireworks-ai" })
        );
        assert_eq!(got[2], ("/api/session/ses_child/agent", r#"{"agent":"cctui-builder"}"#));
    }

    #[tokio::test]
    async fn a_v2_permission_reply_posts_a_decision() {
        let (client, seen) = client_for(Generation::V2).await;
        client.reply_permission("ses_1", "per_1", false).await.unwrap();
        client.reply_permission("ses_1", "per_2", true).await.unwrap();
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen[0].path, "/api/session/ses_1/permission/per_1/reply");
        assert_eq!(seen[0].body, r#"{"decision":"reject"}"#);
        assert_eq!(seen[1].body, r#"{"decision":"once"}"#);
    }

    #[tokio::test]
    async fn a_v2_session_exists_check_reads_the_session() {
        let (client, seen) = client_for(Generation::V2).await;
        client.session_exists("ses_1").await.unwrap();
        assert!(client.session_exists("ses_gone").await.is_err());
        assert_eq!(paths(&seen), vec!["GET /api/session/ses_1", "GET /api/session/ses_gone"]);
    }

    #[test]
    fn events_decode_in_the_clients_generation() {
        let mut client = OpenCodeClient::new("http://x", "pw");
        let v1 = r#"{"type":"session.idle","properties":{"sessionID":"ses_1"}}"#;
        assert!(matches!(
            client.decode_event(v1).unwrap(),
            Incoming::V1(OcEvent::SessionIdle { .. })
        ));
        client.set_generation(Generation::V2);
        let v2 = r#"{"type":"session.execution.succeeded","data":{"sessionID":"ses_1"}}"#;
        let evt = client.decode_event(v2).unwrap();
        assert_eq!(evt.session_id(), Some("ses_1"));
        assert!(matches!(evt, Incoming::V2(v2::events::Event::ExecutionSucceeded(_))));
    }
}
