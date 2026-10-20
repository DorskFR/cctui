//! One dispatched turn on an ACP agent: the core session in one-shot mode,
//! reduced to a verdict once the agent answers `session/prompt`.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_proto::adapter::PermissionMode;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::persist::SessionStore;
use super::rows::AgentRow;
use super::session::{AcpSession, LiveRegistry, SpawnParams};
use crate::dispatch::{self, DispatchAdapter, EventSink, Outcome};

const EVENT_BUFFER: usize = 256;

#[derive(Debug, Clone)]
pub struct OneShot {
    pub row: &'static AgentRow,
    pub bin: String,
    /// The dispatch `SESSION_ID`, the key the server bound the launch to.
    pub key: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub prompt: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub timeout: Duration,
}

impl OneShot {
    /// Build a run from the dispatch payload, resolving the prompt eagerly so
    /// a missing one fails before any process is spawned.
    ///
    /// # Errors
    /// No prompt, or a prompt file that cannot be read.
    pub fn from_payload(row: &'static AgentRow, payload: &Value) -> anyhow::Result<Self> {
        let prompt = crate::dispatch_codex::resolve_dispatch_prompt(payload)?;
        let string = |key: &str| {
            payload
                .get(key)
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
        };
        let mut env = crate::dispatch_codex::payload_env(payload);
        if let Some(aliases) = crate::adapters::gateway_env::gateway_aliases(row.id) {
            aliases.apply(&mut env);
        }
        Ok(Self {
            row,
            bin: std::env::var(format!("CCTUI_{}_BIN", row.id.to_ascii_uppercase()))
                .ok()
                .filter(|b| !b.trim().is_empty())
                .unwrap_or_else(|| row.bin.to_owned()),
            key: std::env::var("SESSION_ID").unwrap_or_default(),
            cwd: std::env::var("CCTUI_DISPATCH_WORKDIR")
                .unwrap_or_else(|_| "/workspace".to_owned()),
            env,
            prompt,
            model: string("model"),
            effort: string("effort"),
            timeout: Duration::from_secs(dispatch::timeout_secs(DispatchAdapter::Acp(row))),
        })
    }

    /// Run the turn to its stop reason and distil the verdict. Events are
    /// forwarded to `sink` under the agent's id so the transcript is posted.
    pub async fn run(self, sink: Option<EventSink>) -> Outcome {
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        let shutdown = CancellationToken::new();
        let store = Arc::new(SessionStore::at(
            std::env::temp_dir().join(format!("cctui-acp-oneshot-{}.json", std::process::id())),
        ));
        let adapter_id = self.row.id;
        let timeout = self.timeout;
        tracing::info!(agent = adapter_id, key = %self.key, "acp dispatch: launching one-shot turn");
        let session =
            AcpSession::new(self.params(), events_tx, LiveRegistry::default(), shutdown.clone())
                .with_store(store)
                .with_reexec(CancellationToken::new())
                .with_oneshot();
        let task = tokio::spawn(session.run());
        let outcome = dispatch::collect(adapter_id, events_rx, sink, timeout).await;
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
        outcome
    }

    fn params(&self) -> SpawnParams {
        SpawnParams {
            row: self.row,
            bin: self.bin.clone(),
            key: self.key.clone(),
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            prompt: Some(self.prompt.clone()),
            name: std::env::var("TASK_NAME").ok().filter(|s| !s.is_empty()),
            model: self.model.clone(),
            effort: self.effort.clone(),
            permission_mode: Some(PermissionMode::Yolo),
            attachments: Vec::new(),
            command_id: None,
            parent_local_id: None,
            agent_mcp: None,
            preflight: None,
            context: Vec::new(),
            resume: None,
            claim: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_payload_binds_the_gateway_aliases_and_reads_model_and_effort() {
        let one = OneShot::from_payload(
            &super::super::rows::GOOSE,
            &json!({
                "adapter": "goose",
                "prompt": "do it",
                "model": "gpt-5.6-sol",
                "effort": "low",
                "env": { "OPENAI_BASE_URL": "https://gw/openai", "OPENAI_API_KEY": "k" },
            }),
        )
        .unwrap();
        assert_eq!(one.prompt, "do it");
        assert_eq!(one.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(one.effort.as_deref(), Some("low"));
        assert_eq!(one.env.get("OPENAI_HOST").map(String::as_str), Some("https://gw/openai"));
        assert_eq!(one.env.get("GOOSE_PROVIDER").map(String::as_str), Some("openai"));
        let params = one.params();
        assert_eq!(params.permission_mode, Some(PermissionMode::Yolo));
        assert!(params.agent_mcp.is_none() && params.preflight.is_none());
    }

    #[test]
    fn from_payload_errors_without_a_prompt() {
        assert!(
            OneShot::from_payload(&super::super::rows::GEMINI, &json!({ "adapter": "gemini" }))
                .is_err()
        );
    }
}
