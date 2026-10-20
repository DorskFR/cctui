//! One dispatched turn on opencode: the session in one-shot mode, reduced to
//! a verdict once its first turn has produced output and gone idle.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_proto::adapter::PermissionMode;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::persist::SessionStore;
use super::session::{LiveRegistry, OpenCodeConfig, OpenCodeSession, SpawnParams};
use crate::dispatch::{self, DispatchAdapter, EventSink, Outcome};

const EVENT_BUFFER: usize = 256;

#[derive(Debug, Clone)]
pub struct OneShot {
    pub cfg: OpenCodeConfig,
    pub key: String,
    pub cwd: String,
    pub env: BTreeMap<String, String>,
    pub prompt: String,
    pub model: String,
    pub agent: Option<String>,
    pub timeout: Duration,
}

impl OneShot {
    /// Build a run from the dispatch payload. opencode has no model of its
    /// own, so `payload.model` is required; the agent profile comes from the
    /// payload env.
    ///
    /// # Errors
    /// No prompt or no model.
    pub fn from_payload(payload: &Value) -> anyhow::Result<Self> {
        let prompt = crate::dispatch_codex::resolve_dispatch_prompt(payload)?;
        let model = payload
            .get("model")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("an opencode dispatch needs payload.model"))?
            .to_owned();
        let env = crate::dispatch_codex::payload_env(payload);
        let agent = env.get(super::AGENT_ENV).cloned().filter(|a| !a.trim().is_empty());
        let mut cfg = OpenCodeConfig::default();
        if let Ok(bin) = std::env::var("CCTUI_OPENCODE_BIN")
            && !bin.trim().is_empty()
        {
            cfg.bin = bin;
        }
        Ok(Self {
            cfg,
            key: std::env::var("SESSION_ID").unwrap_or_default(),
            cwd: std::env::var("CCTUI_DISPATCH_WORKDIR")
                .unwrap_or_else(|_| "/workspace".to_owned()),
            env,
            prompt,
            model,
            agent,
            timeout: Duration::from_secs(dispatch::timeout_secs(DispatchAdapter::OpenCode)),
        })
    }

    /// Run the turn and distil the verdict. Events are forwarded to `sink`
    /// under `opencode` so the transcript is posted.
    pub async fn run(self, sink: Option<EventSink>) -> Outcome {
        let (events_tx, events_rx) = mpsc::channel(EVENT_BUFFER);
        let shutdown = CancellationToken::new();
        let store = Arc::new(SessionStore::at(
            std::env::temp_dir()
                .join(format!("cctui-opencode-oneshot-{}.json", std::process::id())),
        ));
        let timeout = self.timeout;
        tracing::info!(key = %self.key, model = %self.model, "opencode dispatch: launching one-shot turn");
        let session = OpenCodeSession::new(
            self.params(),
            events_tx,
            LiveRegistry::default(),
            shutdown.clone(),
        )
        .with_store(store)
        .with_oneshot();
        let task = tokio::spawn(session.run());
        let outcome = dispatch::collect(super::ADAPTER_ID, events_rx, sink, timeout).await;
        shutdown.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(10), task).await;
        outcome
    }

    fn params(&self) -> SpawnParams {
        SpawnParams {
            cfg: self.cfg.clone(),
            key: self.key.clone(),
            cwd: self.cwd.clone(),
            env: self.env.clone(),
            prompt: Some(self.prompt.clone()),
            name: std::env::var("TASK_NAME").ok().filter(|s| !s.is_empty()),
            model: Some(self.model.clone()),
            agent: self.agent.clone(),
            attachments: Vec::new(),
            command_id: None,
            parent_local_id: None,
            agent_mcp: None,
            skill_roots: Vec::new(),
            context: Vec::new(),
            preflight: None,
            permission_mode: Some(PermissionMode::Yolo),
            resume: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn from_payload_needs_a_model_and_reads_the_agent_from_the_env() {
        let one = OneShot::from_payload(&json!({
            "adapter": "opencode",
            "prompt": "review",
            "model": "fireworks/kimi-k2",
            "env": { "CCTUI_OPENCODE_AGENT": "reviewer", "FIREWORKS_API_KEY": "k" },
        }))
        .unwrap();
        assert_eq!(super::super::AGENT_ENV, "CCTUI_OPENCODE_AGENT");
        assert_eq!(one.model, "fireworks/kimi-k2");
        assert_eq!(one.agent.as_deref(), Some("reviewer"));
        assert_eq!(one.env.get("FIREWORKS_API_KEY").map(String::as_str), Some("k"));
        let params = one.params();
        assert_eq!(params.permission_mode, Some(PermissionMode::Yolo));
        assert_eq!(params.model.as_deref(), Some("fireworks/kimi-k2"));

        let err = OneShot::from_payload(&json!({ "adapter": "opencode", "prompt": "x" }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("payload.model"), "{err}");
        assert!(OneShot::from_payload(&json!({ "adapter": "opencode", "model": "m" })).is_err());
    }
}
