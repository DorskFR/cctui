//! Completion tracking for `CctuiAgent`-spawned child sessions.
//!
//! The `CctuiAgent` tool blocks until its child finishes, but the daemon has no
//! request/response channel to a session — it only sees the child's
//! [`AdapterEvent`] stream on its way to the server. The supervisor feeds every
//! event through [`ChildWatch::observe`], which keeps the last assistant text
//! per watched child and resolves the waiter when the child ends.
//!
//! A child is matched either by its pre-minted session id (claude launches with
//! it as `--session-id`) or, for adapters that mint their own local id, by the
//! `parent_local_id` on its `SessionStarted`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use cctui_proto::adapter::{AdapterEvent, EndReason};
use tokio::sync::oneshot;

/// How a watched child finished.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChildOutcome {
    /// The child's last assistant message, when it produced one.
    pub final_text: Option<String>,
    /// Failure text when the child never started or ended badly.
    pub error: Option<String>,
}

struct Watch {
    parent: String,
    /// The adapter-side id once observed; until then only `key` is known.
    local_id: Option<String>,
    final_text: Option<String>,
    done: Option<oneshot::Sender<ChildOutcome>>,
}

impl Watch {
    fn matches(&self, key: &str, local_id: &str) -> bool {
        self.local_id.as_deref() == Some(local_id) || key == local_id
    }
}

#[derive(Default)]
pub struct ChildWatch {
    watches: Mutex<HashMap<String, Watch>>,
}

static GLOBAL: OnceLock<Arc<ChildWatch>> = OnceLock::new();

/// The process-wide watcher: written by the supervisor's event pump, read by the
/// agent-tool handler.
pub fn global() -> Arc<ChildWatch> {
    GLOBAL.get_or_init(|| Arc::new(ChildWatch::default())).clone()
}

impl ChildWatch {
    /// Start watching the child `key` (its pre-minted session id) spawned by
    /// `parent`. The receiver resolves once the child ends.
    pub fn register(&self, key: &str, parent: &str) -> oneshot::Receiver<ChildOutcome> {
        let (tx, rx) = oneshot::channel();
        let watch =
            Watch { parent: parent.to_owned(), local_id: None, final_text: None, done: Some(tx) };
        self.watches.lock().unwrap().insert(key.to_owned(), watch);
        rx
    }

    /// Stop watching `key` without resolving it (the caller gave up).
    pub fn cancel(&self, key: &str) {
        self.watches.lock().unwrap().remove(key);
    }

    /// Feed one adapter event. Cheap no-op while nothing is being watched.
    pub fn observe(&self, event: &AdapterEvent) {
        let Ok(mut guard) = self.watches.lock() else { return };
        if guard.is_empty() {
            drop(guard);
            return;
        }
        let watches = &mut *guard;
        match event {
            AdapterEvent::SessionStarted { local_id, meta } => {
                bind(watches, local_id, meta.parent_local_id.as_deref());
            }
            AdapterEvent::Message { local_id, payload } => {
                if let Some(text) = assistant_text(payload)
                    && let Some(w) = find_mut(watches, local_id)
                {
                    w.final_text = Some(text);
                }
            }
            AdapterEvent::SessionEnded { local_id, reason } => {
                let Some(key) = find_key(watches, local_id) else { return };
                let outcome = match reason {
                    EndReason::Crashed { detail } | EndReason::Other { detail } => {
                        Some(detail.clone())
                    }
                    _ => None,
                };
                resolve(watches, &key, outcome);
            }
            AdapterEvent::CommandResult { command_id, ok, error } if !ok => {
                let key = command_id.to_string();
                let error = error.clone().unwrap_or_else(|| "spawn failed".to_owned());
                resolve(watches, &key, Some(error));
            }
            _ => {}
        }
    }
}

/// Attach an adapter-minted `local_id` to the watch it belongs to: by exact key
/// first, else by the parent it reported.
fn bind(watches: &mut HashMap<String, Watch>, local_id: &str, parent: Option<&str>) {
    if let Some(w) = watches.get_mut(local_id) {
        w.local_id = Some(local_id.to_owned());
        return;
    }
    let Some(parent) = parent else { return };
    if let Some(w) = watches.values_mut().find(|w| w.local_id.is_none() && w.parent == parent) {
        w.local_id = Some(local_id.to_owned());
    }
}

fn find_key(watches: &HashMap<String, Watch>, local_id: &str) -> Option<String> {
    watches.iter().find(|(k, w)| w.matches(k, local_id)).map(|(k, _)| k.clone())
}

fn find_mut<'a>(watches: &'a mut HashMap<String, Watch>, local_id: &str) -> Option<&'a mut Watch> {
    let key = find_key(watches, local_id)?;
    watches.get_mut(&key)
}

fn resolve(watches: &mut HashMap<String, Watch>, key: &str, error: Option<String>) {
    let Some(mut watch) = watches.remove(key) else { return };
    if let Some(done) = watch.done.take() {
        let _ = done.send(ChildOutcome { final_text: watch.final_text.take(), error });
    }
}

/// The text of an assistant message, ignoring thinking blocks and user turns.
fn assistant_text(payload: &serde_json::Value) -> Option<String> {
    if payload.get("role").and_then(serde_json::Value::as_str) != Some("assistant") {
        return None;
    }
    let text = payload.get("text").and_then(serde_json::Value::as_str)?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::adapter::SessionMeta;
    use serde_json::json;

    fn started(local_id: &str, parent: Option<&str>) -> AdapterEvent {
        AdapterEvent::SessionStarted {
            local_id: local_id.to_owned(),
            meta: SessionMeta {
                parent_local_id: parent.map(str::to_owned),
                ..SessionMeta::default()
            },
        }
    }

    fn msg(local_id: &str, role: &str, text: &str) -> AdapterEvent {
        AdapterEvent::Message {
            local_id: local_id.to_owned(),
            payload: json!({ "role": role, "text": text }),
        }
    }

    #[tokio::test]
    async fn child_matched_by_pre_minted_id_returns_its_last_assistant_text() {
        let watch = ChildWatch::default();
        let rx = watch.register("child-1", "parent-1");
        watch.observe(&started("child-1", Some("parent-1")));
        watch.observe(&msg("child-1", "assistant", "first pass"));
        watch.observe(&msg("child-1", "assistant", "VERDICT: approve"));
        watch.observe(&AdapterEvent::SessionEnded {
            local_id: "child-1".into(),
            reason: EndReason::Completed,
        });
        let outcome = rx.await.unwrap();
        assert_eq!(outcome.final_text.as_deref(), Some("VERDICT: approve"));
        assert!(outcome.error.is_none());
    }

    #[tokio::test]
    async fn child_with_its_own_local_id_is_matched_by_parent() {
        let watch = ChildWatch::default();
        let rx = watch.register("child-key", "parent-1");
        watch.observe(&started("opencode-abc", Some("parent-1")));
        watch.observe(&msg("opencode-abc", "assistant", "done"));
        watch.observe(&AdapterEvent::SessionEnded {
            local_id: "opencode-abc".into(),
            reason: EndReason::Completed,
        });
        assert_eq!(rx.await.unwrap().final_text.as_deref(), Some("done"));
    }

    #[tokio::test]
    async fn unrelated_sessions_never_resolve_a_watch() {
        let watch = ChildWatch::default();
        let mut rx = watch.register("child-1", "parent-1");
        watch.observe(&started("someone-else", Some("other-parent")));
        watch.observe(&msg("someone-else", "assistant", "not mine"));
        watch.observe(&AdapterEvent::SessionEnded {
            local_id: "someone-else".into(),
            reason: EndReason::Completed,
        });
        assert!(rx.try_recv().is_err(), "watch must still be pending");
    }

    #[tokio::test]
    async fn crashed_child_reports_the_failure_detail() {
        let watch = ChildWatch::default();
        let rx = watch.register("child-1", "parent-1");
        watch.observe(&started("child-1", Some("parent-1")));
        watch.observe(&AdapterEvent::SessionEnded {
            local_id: "child-1".into(),
            reason: EndReason::Crashed { detail: "binary missing".into() },
        });
        let outcome = rx.await.unwrap();
        assert_eq!(outcome.error.as_deref(), Some("binary missing"));
    }

    #[tokio::test]
    async fn failed_spawn_command_resolves_immediately() {
        let watch = ChildWatch::default();
        let id = uuid::Uuid::new_v4();
        let rx = watch.register(&id.to_string(), "parent-1");
        watch.observe(&AdapterEvent::CommandResult {
            command_id: id,
            ok: false,
            error: Some("adapter offline".into()),
        });
        assert_eq!(rx.await.unwrap().error.as_deref(), Some("adapter offline"));
    }

    #[test]
    fn thinking_and_user_messages_are_not_the_final_text() {
        assert!(assistant_text(&json!({ "role": "assistant_thinking", "text": "hmm" })).is_none());
        assert!(assistant_text(&json!({ "role": "user", "text": "go" })).is_none());
        assert!(assistant_text(&json!({ "role": "assistant", "text": "  " })).is_none());
        assert_eq!(
            assistant_text(&json!({ "role": "assistant", "text": " ok " })).as_deref(),
            Some("ok")
        );
    }
}
