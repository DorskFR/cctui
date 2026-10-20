//! v2 endpoints on the shared [`OpenCodeClient`] transport. Responses come
//! wrapped in `{ data: … }`, and a session's model and agent live on the
//! session rather than on each prompt.

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::adapters::opencode::client::{OpenCodeClient, SessionModelRef, Tokens};

pub const PINNED_VERSION: &str = "2.0.27";

/// A server that keeps handing back a cursor must not loop the reader forever.
const MAX_MESSAGE_PAGES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Info {
    pub version: String,
    #[serde(default)]
    pub pid: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Data<T> {
    pub data: T,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct Session {
    pub id: String,
    #[serde(default, rename = "parentID")]
    pub parent_id: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Location {
    pub directory: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CreateSession {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<SessionModelRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<Location>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Delivery {
    Steer,
    Queue,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptRequest {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery: Option<Delivery>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Once,
    Always,
    Reject,
}

/// One transcript item; only the kinds the adapter reads are typed.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    User {
        id: String,
        #[serde(default)]
        text: String,
    },
    Assistant {
        id: String,
        #[serde(default)]
        tokens: Option<Tokens>,
        #[serde(default)]
        cost: Option<f64>,
        #[serde(default)]
        finish: Option<String>,
    },
    Idle {
        id: String,
        #[serde(default)]
        outcome: String,
    },
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Cursor {
    #[serde(default)]
    pub next: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessagePage {
    pub data: Vec<Message>,
    #[serde(default)]
    pub cursor: Cursor,
}

fn session_path(session_id: &str, tail: &str) -> String {
    format!("/api/session/{session_id}{tail}")
}

fn is_json(resp: &reqwest::Response) -> bool {
    resp.headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.contains("json"))
}

impl OpenCodeClient {
    /// `None` when the answer is not JSON, as from a v1 server.
    pub async fn v2_info(&self) -> Result<Option<Info>> {
        let resp = self.send(self.get("/api/info"), "GET", "/api/info", None).await?;
        if !is_json(&resp) {
            return Ok(None);
        }
        Ok(Some(resp.json().await?))
    }

    pub async fn v2_create_session(&self, body: &CreateSession) -> Result<Session> {
        let json = serde_json::to_string(body).unwrap_or_default();
        let resp = self
            .send(self.post("/api/session").json(body), "POST", "/api/session", Some(&json))
            .await?;
        Ok(resp.json::<Data<Session>>().await?.data)
    }

    pub async fn v2_session(&self, session_id: &str) -> Result<Session> {
        let path = session_path(session_id, "");
        let resp = self.send(self.get(&path), "GET", &path, None).await?;
        Ok(resp.json::<Data<Session>>().await?.data)
    }

    /// Returns once the prompt is queued; the turn is observed on the bus.
    pub async fn v2_prompt(&self, session_id: &str, body: &PromptRequest) -> Result<()> {
        let path = session_path(session_id, "/prompt");
        let json = serde_json::to_string(body).unwrap_or_default();
        self.send(self.post(&path).json(body), "POST", &path, Some(&json)).await?;
        Ok(())
    }

    pub async fn v2_set_model(&self, session_id: &str, model: &SessionModelRef) -> Result<()> {
        let path = session_path(session_id, "/model");
        let body = serde_json::json!({ "model": model });
        self.send(self.post(&path).json(&body), "POST", &path, Some(&body.to_string())).await?;
        Ok(())
    }

    pub async fn v2_set_agent(&self, session_id: &str, agent: &str) -> Result<()> {
        let path = session_path(session_id, "/agent");
        let body = serde_json::json!({ "agent": agent });
        self.send(self.post(&path).json(&body), "POST", &path, Some(&body.to_string())).await?;
        Ok(())
    }

    pub async fn v2_messages(&self, session_id: &str) -> Result<Vec<Message>> {
        let path = session_path(session_id, "/message");
        let mut out = Vec::new();
        let mut request = self.get(&path).query(&[("order", "asc")]);
        for _ in 0..MAX_MESSAGE_PAGES {
            let page: MessagePage = self.send(request, "GET", &path, None).await?.json().await?;
            out.extend(page.data);
            let Some(next) = page.cursor.next.filter(|c| !c.is_empty()) else { break };
            request = self.get(&path).query(&[("cursor", next.as_str())]);
        }
        Ok(out)
    }

    pub async fn v2_interrupt(&self, session_id: &str) -> Result<()> {
        let path = session_path(session_id, "/interrupt");
        self.send(self.post(&path), "POST", &path, None).await?;
        Ok(())
    }

    pub async fn v2_fork(&self, session_id: &str) -> Result<Session> {
        let path = session_path(session_id, "/fork");
        let resp =
            self.send(self.post(&path).json(&serde_json::json!({})), "POST", &path, None).await?;
        Ok(resp.json::<Data<Session>>().await?.data)
    }

    pub async fn v2_reply_permission(
        &self,
        session_id: &str,
        request_id: &str,
        decision: Decision,
    ) -> Result<()> {
        let path = session_path(session_id, &format!("/permission/{request_id}/reply"));
        let body = serde_json::json!({ "decision": decision });
        self.send(self.post(&path).json(&body), "POST", &path, Some(&body.to_string())).await?;
        Ok(())
    }

    /// `Last-Event-ID` is ignored: a reconnect resumes from now, not from
    /// where the stream dropped.
    pub async fn v2_events(&self) -> Result<reqwest::Response> {
        let request =
            self.stream_get("/api/event").header(reqwest::header::ACCEPT, "text/event-stream");
        self.send(request, "GET", "/api/event", None).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::opencode::fake_serve::{self, Reply};

    #[test]
    fn info_round_trips() {
        let raw = r#"{"version":"2.0.27","pid":4242,"urls":["http://127.0.0.1:4096"],
                      "paths":{"tmp":"/tmp/opencode"},"capabilities":null}"#;
        let info: Info = serde_json::from_str(raw).unwrap();
        assert_eq!(info, Info { version: PINNED_VERSION.to_owned(), pid: Some(4242) });
    }

    #[test]
    fn a_session_unwraps_from_its_data_envelope() {
        let raw = r#"{"data":{"id":"ses_ecd86c6adffeSeDX15STTLCF8s","projectID":"p","cost":0,
            "tokens":{"input":0,"output":0,"reasoning":0,"cache":{"read":0,"write":0}},
            "time":{"created":1,"updated":1},"location":{"directory":"/work"},"title":"t",
            "agent":"cctui-reviewer","model":{"id":"mock","providerID":"fireworks-ai"}}}"#;
        let s = serde_json::from_str::<Data<Session>>(raw).unwrap().data;
        assert_eq!(s.id, "ses_ecd86c6adffeSeDX15STTLCF8s");
        assert_eq!(s.title.as_deref(), Some("t"));
        assert_eq!(s.parent_id, None);
    }

    #[test]
    fn create_session_omits_unset_fields() {
        let v = serde_json::to_value(CreateSession {
            title: Some("PR review".to_owned()),
            location: Some(Location { directory: "/repo".to_owned() }),
            ..CreateSession::default()
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "title": "PR review", "location": { "directory": "/repo" } })
        );
    }

    #[test]
    fn prompt_and_decision_serialize_to_the_documented_shape() {
        let body = PromptRequest { text: "hi".to_owned(), delivery: Some(Delivery::Queue) };
        assert_eq!(
            serde_json::to_value(body).unwrap(),
            serde_json::json!({ "text": "hi", "delivery": "queue" })
        );
        assert_eq!(serde_json::to_value(Decision::Always).unwrap(), "always");
    }

    #[test]
    fn transcript_items_are_typed_by_kind() {
        let raw = serde_json::json!([
            { "id": "msg_0", "type": "user", "text": "list the files", "time": { "created": 1 } },
            { "id": "msg_1", "type": "assistant", "time": { "created": 2 }, "content": [],
              "finish": "stop", "cost": 0.5,
              "tokens": { "input": 90, "output": 10, "reasoning": 0, "cache": { "read": 0, "write": 0 } } },
            { "id": "msg_2", "type": "idle", "outcome": "succeeded", "time": { "created": 3 } },
            { "id": "msg_3", "type": "model-switched", "time": { "created": 4 } }
        ]);
        let items: Vec<Message> = serde_json::from_value(raw).unwrap();
        assert!(matches!(&items[0], Message::User { text, .. } if text == "list the files"));
        match &items[1] {
            Message::Assistant { tokens, finish, .. } => {
                assert_eq!(tokens.unwrap().input, 90);
                assert_eq!(finish.as_deref(), Some("stop"));
            }
            other => panic!("expected an assistant item, got {other:?}"),
        }
        assert!(matches!(&items[2], Message::Idle { outcome, .. } if outcome == "succeeded"));
        assert_eq!(items[3], Message::Other);
    }

    fn paged(_method: &str, path: &str) -> Reply {
        match path {
            "/api/session/ses_1/message?order=asc" => Reply::json(
                r#"{"data":[{"id":"msg_0","type":"user","text":"a","time":{"created":1}}],
                    "cursor":{"previous":null,"next":"c2"}}"#,
            ),
            "/api/session/ses_1/message?cursor=c2" => Reply::json(
                r#"{"data":[{"id":"msg_1","type":"idle","outcome":"succeeded","time":{"created":2}}],
                    "cursor":{"previous":"c1","next":null}}"#,
            ),
            _ => Reply::status(404),
        }
    }

    #[tokio::test]
    async fn the_transcript_is_read_page_by_page() {
        let (base, seen) = fake_serve::serve(paged).await;
        let items = OpenCodeClient::new(base, "pw").v2_messages("ses_1").await.unwrap();
        assert_eq!(items.len(), 2);
        assert!(matches!(&items[1], Message::Idle { id, .. } if id == "msg_1"));
        let paths: Vec<String> = seen.lock().unwrap().iter().map(|s| s.path.clone()).collect();
        assert_eq!(
            paths,
            vec!["/api/session/ses_1/message?order=asc", "/api/session/ses_1/message?cursor=c2"]
        );
    }

    #[tokio::test]
    async fn a_v1_web_page_on_api_info_is_no_v2_answer() {
        let (base, _seen) = fake_serve::serve(|_, _| Reply::html()).await;
        assert_eq!(OpenCodeClient::new(base, "pw").v2_info().await.unwrap(), None);
    }

    #[tokio::test]
    async fn the_event_stream_is_read_from_the_v2_path() {
        let (base, seen) = fake_serve::serve(|_, _| Reply::status(200)).await;
        OpenCodeClient::new(base, "pw").v2_events().await.unwrap();
        assert_eq!(seen.lock().unwrap()[0].path, "/api/event");
    }
}
