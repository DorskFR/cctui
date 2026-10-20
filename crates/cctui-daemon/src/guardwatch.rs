//! Mirror of the pod's workflow guard (`cctui-guard`).
//!
//! Polls `GET /state` and relays every change up as
//! [`AdapterEvent::GuardStepState`]; forwards a server-requested
//! [`AdapterCommand::GuardTransition`] to `POST /transition`. The guard stays
//! authoritative: nothing here decides a step.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use std::time::Duration;

use cctui_proto::adapter::{AdapterCommand, AdapterEvent};
use cctui_proto::guard_step::{GuardStepState, parse_state, transition_target};
use tokio::sync::{Notify, mpsc};
use tokio_util::sync::CancellationToken;

/// Set by the worker entrypoint when a guard runs in the pod.
pub const URL_ENV: &str = "CCTUI_GUARD_URL";

const POLL: Duration = Duration::from_secs(3);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

static GLOBAL: OnceLock<GuardWatch> = OnceLock::new();

#[must_use]
pub fn global() -> &'static GuardWatch {
    GLOBAL.get_or_init(GuardWatch::default)
}

/// The guard's base URL, when this daemon runs beside one.
#[must_use]
pub fn url() -> Option<String> {
    std::env::var(URL_ENV)
        .ok()
        .map(|u| u.trim().trim_end_matches('/').to_owned())
        .filter(|u| !u.is_empty())
}

/// Live sessions on this daemon (`local_id` → `adapter_id`) and the last
/// state relayed, so only changes go up.
#[derive(Default)]
pub struct GuardWatch {
    sessions: Mutex<HashMap<String, String>>,
    last: Mutex<Option<GuardStepState>>,
    nudge: Notify,
}

impl GuardWatch {
    /// Track which sessions are live; a new one gets the current state
    /// relayed right away, whether or not it changed.
    pub fn observe(&self, adapter_id: &str, event: &AdapterEvent) {
        match event {
            AdapterEvent::SessionStarted { local_id, .. } => {
                self.sessions
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .insert(local_id.clone(), adapter_id.to_owned());
                self.nudge.notify_one();
            }
            AdapterEvent::SessionEnded { local_id, .. } => {
                self.sessions.lock().unwrap_or_else(PoisonError::into_inner).remove(local_id);
            }
            _ => {}
        }
    }

    /// The events to send for `state`: one per live session when it differs
    /// from the last relayed state or `force` is set, none otherwise.
    pub fn relay(&self, state: &GuardStepState, force: bool) -> Vec<(String, AdapterEvent)> {
        {
            let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
            if !force && last.as_ref() == Some(state) {
                return Vec::new();
            }
            *last = Some(state.clone());
        }
        self.sessions
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(local_id, adapter_id)| {
                (
                    adapter_id.clone(),
                    AdapterEvent::GuardStepState {
                        local_id: local_id.clone(),
                        state: state.clone(),
                    },
                )
            })
            .collect()
    }

    #[cfg(test)]
    fn reset(&self) {
        self.sessions.lock().unwrap_or_else(PoisonError::into_inner).clear();
        *self.last.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }
}

fn client() -> Option<reqwest::Client> {
    reqwest::Client::builder()
        .no_proxy()
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|err| tracing::warn!(%err, "guard http client"))
        .ok()
}

/// Start the poll loop when the pod has a guard; a no-op otherwise.
pub fn start(event_tx: mpsc::Sender<(String, AdapterEvent)>, shutdown: CancellationToken) {
    let Some(url) = url() else { return };
    let Some(client) = client() else { return };
    tracing::info!(%url, "workflow guard mirror on");
    tokio::spawn(async move {
        loop {
            let nudged = tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(POLL) => false,
                () = global().nudge.notified() => true,
            };
            match fetch_state(&client, &url).await {
                Ok(state) => {
                    for pair in global().relay(&state, nudged) {
                        if event_tx.send(pair).await.is_err() {
                            return;
                        }
                    }
                }
                Err(err) => tracing::debug!(%err, "guard state poll"),
            }
        }
    });
}

/// `GET /state`.
pub async fn fetch_state(client: &reqwest::Client, url: &str) -> Result<GuardStepState, String> {
    let body: serde_json::Value = client
        .get(format!("{url}/state"))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    parse_state(&body).ok_or_else(|| format!("guard state without a step: {body}"))
}

/// `POST /transition` with the guard's own verdict as the result.
pub async fn transition(client: &reqwest::Client, url: &str, step: &str) -> Result<(), String> {
    let body: serde_json::Value = client
        .post(format!("{url}/transition"))
        .json(&serde_json::json!({ "step": transition_target(step) }))
        .send()
        .await
        .map_err(|e| e.to_string())?
        .json()
        .await
        .map_err(|e| e.to_string())?;
    transition_outcome(&body)
}

/// The guard answers `{"ok": bool, "error"?: string}`.
pub fn transition_outcome(body: &serde_json::Value) -> Result<(), String> {
    if body.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(body
        .get("error")
        .and_then(serde_json::Value::as_str)
        .map_or_else(|| format!("guard refused the transition: {body}"), str::to_owned))
}

/// Serve a [`AdapterCommand::GuardTransition`] off the transport loop: the
/// verdict goes back as a `CommandResult`, then the fresh state is relayed.
/// Returns `false` for any other command.
#[must_use]
pub fn handle_command(
    adapter_id: &str,
    command: &AdapterCommand,
    event_tx: &mpsc::Sender<(String, AdapterEvent)>,
) -> bool {
    let AdapterCommand::GuardTransition { local_id, step, command_id } = command else {
        return false;
    };
    let adapter_id = adapter_id.to_owned();
    let (local_id, step, command_id) = (local_id.clone(), step.clone(), *command_id);
    let event_tx = event_tx.clone();
    tokio::spawn(async move {
        let outcome = match (url(), client()) {
            (Some(url), Some(client)) => {
                let result = transition(&client, &url, &step).await;
                if let Ok(state) = fetch_state(&client, &url).await {
                    for pair in global().relay(&state, true) {
                        let _ = event_tx.send(pair).await;
                    }
                }
                result
            }
            _ => Err("this worker runs no workflow guard".to_owned()),
        };
        match &outcome {
            Ok(()) => tracing::info!(%local_id, %step, "guard transition accepted"),
            Err(err) => tracing::warn!(%local_id, %step, %err, "guard transition refused"),
        }
        if let Some(command_id) = command_id {
            let event = AdapterEvent::CommandResult {
                command_id,
                ok: outcome.is_ok(),
                error: outcome.err(),
            };
            let _ = event_tx.send((adapter_id, event)).await;
        }
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::adapter::{EndReason, SessionMeta};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn state(step: i64) -> GuardStepState {
        GuardStepState {
            step,
            title: format!("step {step}"),
            transitions: vec![2],
            ..Default::default()
        }
    }

    #[test]
    fn relays_only_changes_to_every_live_session_and_forces_on_demand() {
        let w = GuardWatch::default();
        assert!(w.relay(&state(1), false).is_empty(), "no live session, nothing to relay");

        w.observe(
            "claude-code",
            &AdapterEvent::SessionStarted { local_id: "s1".into(), meta: SessionMeta::default() },
        );
        w.observe(
            "codex",
            &AdapterEvent::SessionStarted { local_id: "s2".into(), meta: SessionMeta::default() },
        );
        assert!(w.relay(&state(1), false).is_empty(), "same state as last relayed");

        let mut out = w.relay(&state(2), false);
        out.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].0, "claude-code");
        match &out[0].1 {
            AdapterEvent::GuardStepState { local_id, state } => {
                assert_eq!(local_id, "s1");
                assert_eq!(state.step, 2);
            }
            other => panic!("expected GuardStepState, got {other:?}"),
        }
        assert_eq!(w.relay(&state(2), true).len(), 2, "forced: a new session needs the mirror");

        w.observe(
            "codex",
            &AdapterEvent::SessionEnded { local_id: "s2".into(), reason: EndReason::Completed },
        );
        assert_eq!(w.relay(&state(3), false).len(), 1);
    }

    #[test]
    fn a_transition_reply_is_the_guard_verdict() {
        assert_eq!(transition_outcome(&json!({ "ok": true, "step": 2 })), Ok(()));
        assert_eq!(
            transition_outcome(
                &json!({ "ok": false, "step": 1, "error": "Step 4 is not a valid transition" })
            ),
            Err("Step 4 is not a valid transition".to_owned())
        );
        assert!(transition_outcome(&json!({})).unwrap_err().contains("refused"));
    }

    #[test]
    fn a_foreign_command_is_not_handled() {
        let (tx, _rx) = mpsc::channel(1);
        let cmd = AdapterCommand::WatchText { local_id: "s1".into(), watch: true };
        assert!(!handle_command("claude-code", &cmd, &tx));
    }

    fn request_complete(raw: &[u8]) -> bool {
        let text = String::from_utf8_lossy(raw);
        let Some(head_end) = text.find("\r\n\r\n") else { return false };
        let body_len = text[..head_end]
            .lines()
            .find_map(|l| {
                l.strip_prefix("content-length:").or_else(|| l.strip_prefix("Content-Length:"))
            })
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        raw.len() >= head_end + 4 + body_len
    }

    /// A one-request guard stand-in: answers `body` to whatever arrives and
    /// hands the whole request back.
    async fn fake_guard(body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut raw = Vec::new();
            let mut chunk = [0u8; 1024];
            // Headers and body may land in separate segments.
            while !request_complete(&raw) {
                let n = sock.read(&mut chunk).await.unwrap();
                assert!(n > 0, "guard stand-in: connection closed mid-request");
                raw.extend_from_slice(&chunk[..n]);
            }
            let req = String::from_utf8_lossy(&raw).into_owned();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            req
        });
        (url, task)
    }

    #[tokio::test]
    async fn fetches_and_parses_the_guard_state() {
        let (url, task) = fake_guard(r#"{"step":2,"title":"Implement","allowed":"all-read","disallowed":"","transitions":[3],"exit":true}"#).await;
        let got = fetch_state(&client().unwrap(), &url).await.unwrap();
        assert_eq!(got.step, 2);
        assert_eq!(got.title, "Implement");
        assert_eq!(got.transitions, vec![3]);
        assert!(got.exit);
        assert!(task.await.unwrap().starts_with("GET /state "));
    }

    #[tokio::test]
    async fn posts_the_transition_as_the_guard_expects() {
        let (url, task) = fake_guard(r#"{"ok":true,"step":3}"#).await;
        transition(&client().unwrap(), &url, "3").await.unwrap();
        let req = task.await.unwrap();
        assert!(req.starts_with("POST /transition "));
        assert!(req.ends_with(r#"{"step":3}"#), "numeric target as a JSON number: {req}");

        let (url, task) = fake_guard(r#"{"ok":false,"step":3,"error":"gate failed"}"#).await;
        assert_eq!(
            transition(&client().unwrap(), &url, "exit").await,
            Err("gate failed".to_owned())
        );
        assert!(task.await.unwrap().ends_with(r#"{"step":"exit"}"#));
    }

    #[tokio::test]
    async fn a_transition_command_answers_with_the_verdict_then_the_state() {
        global().reset();
        global().observe(
            "claude-code",
            &AdapterEvent::SessionStarted { local_id: "g1".into(), meta: SessionMeta::default() },
        );
        let (tx, mut rx) = mpsc::channel(8);
        let id = uuid::Uuid::new_v4();
        let cmd = AdapterCommand::GuardTransition {
            local_id: "g1".into(),
            step: "2".into(),
            command_id: Some(id),
        };
        // No CCTUI_GUARD_URL in the test env: the command is still served, as a refusal.
        assert!(handle_command("claude-code", &cmd, &tx));
        let (adapter_id, event) = rx.recv().await.unwrap();
        assert_eq!(adapter_id, "claude-code");
        match event {
            AdapterEvent::CommandResult { command_id, ok, error } => {
                assert_eq!(command_id, id);
                assert!(!ok);
                assert!(error.unwrap().contains("no workflow guard"));
            }
            other => panic!("expected CommandResult, got {other:?}"),
        }
        global().reset();
    }
}
