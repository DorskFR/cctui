//! Shared dispatch abstraction + the helpers every platform dispatcher needs.
//!
//! A concrete dispatcher (docker `HostConfig`, kube `PodSpec`, apple plist)
//! implements [`Dispatcher`]; [`crate::run::Runner`] drives it over the wire.
//! The helpers here (`worker_name`, `dedup_source`, `label_safe`, `build_env`)
//! are shared by every platform dispatcher.

use std::future::Future;

use cctui_proto::ws::WireDispatchSpec;
use sha1::{Digest, Sha1};

/// Lifecycle state of a spawned worker handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandleState {
    Running,
    /// Created but held back by a concurrency cap (kube: `spec.suspend`).
    /// Servers without a dedicated mapping treat it as `Running`.
    Queued,
    Complete,
    Failed,
    Gone,
}

impl HandleState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Queued => "queued",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Gone => "gone",
        }
    }
}

/// Outcome of a dispatch: an opaque handle, the idempotency status reported back
/// to the server verbatim, and an optional namespace (kube sets it; docker and
/// apple leave it `None`).
#[derive(Debug, Clone)]
pub struct SpawnOutcome {
    pub handle: String,
    pub status: String,
    pub namespace: Option<String>,
}

/// A platform executor: a running worker reported/cancelled by opaque handle.
///
/// `dispatch` builds the platform-specific workload (`HostConfig` / `PodSpec` /
/// plist); everything else — the WS loop, reconnect, framing — lives in
/// [`crate::run::Runner`].
pub trait Dispatcher: Send + Sync {
    /// Wire `kind` announced in the `Hello` frame (`docker`/`kubernetes`/`apple`).
    fn kind(&self) -> &'static str;

    fn dispatch(
        &self,
        spec: &WireDispatchSpec,
    ) -> impl Future<Output = anyhow::Result<SpawnOutcome>> + Send;

    fn status(
        &self,
        handle: &str,
    ) -> impl Future<Output = anyhow::Result<(HandleState, Option<String>)>> + Send;

    fn cancel(&self, handle: &str) -> impl Future<Output = anyhow::Result<()>> + Send;
}

/// `cctui-worker-<sha1(dedup_source)[:12]>` — deterministic so a repeat dispatch
/// of the same key maps to the same worker (idempotency key).
#[must_use]
pub fn worker_name(dedup_source: &str) -> String {
    let digest = Sha1::digest(dedup_source.as_bytes());
    format!("cctui-worker-{}", &hex::encode(digest)[..12])
}

/// The string the worker name derives from.
///
/// The caller's `dedup_key` (the logical request id) when present, else the
/// `session_id`. Keeping idempotency on the dedup key lets `session_id` be fresh
/// per dispatch while a repeat of the same logical key still coalesces onto one
/// worker.
#[must_use]
pub fn dedup_source(spec: &WireDispatchSpec) -> &str {
    spec.dedup_key.as_deref().filter(|k| !k.is_empty()).unwrap_or(&spec.session_id)
}

/// Coerce an arbitrary string into a valid k8s/docker label value.
///
/// `[A-Za-z0-9_.-]`, truncated to 63 chars, trimmed of leading/trailing
/// separators, with a stable fallback when the result is empty. Case is
/// preserved.
#[must_use]
pub fn label_safe(value: &str) -> String {
    let mapped: String = value
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '-' })
        .collect();
    let truncated: String = mapped.chars().take(63).collect();
    let trimmed = truncated.trim_matches(|c| matches!(c, '-' | '_' | '.'));
    if trimmed.is_empty() { "session".to_owned() } else { trimmed.to_owned() }
}

/// The base worker env shared by every dispatcher.
///
/// The session identifiers, the payload (with `cctui_machine_key` lifted out),
/// the dial-back URL, and the optional task name / reply URL. The machine key is
/// returned separately so a dispatcher can inject it however it wants (env var
/// vs. mounted file).
#[derive(Debug, Clone)]
pub struct BaseEnv {
    pub env: Vec<String>,
    pub machine_key: Option<String>,
}

/// Build [`BaseEnv`] from a dispatch spec.
///
/// `cctui_machine_key` is removed from the payload (runtime identity, never
/// persisted into `TASK_PAYLOAD_JSON`) and returned as `machine_key`; `name` is
/// read for `TASK_NAME` but left in the payload.
pub fn build_env(spec: &WireDispatchSpec, cctui_url: &str) -> anyhow::Result<BaseEnv> {
    let mut payload = spec.payload.clone();
    let machine_key = payload
        .as_object_mut()
        .and_then(|o| o.remove("cctui_machine_key"))
        .and_then(|v| v.as_str().map(ToOwned::to_owned));
    let task_name = payload.get("name").and_then(|v| v.as_str()).map(ToOwned::to_owned);
    let payload_json = serde_json::to_string(&payload)?;

    let mut env = vec![
        format!("SESSION_ID={}", spec.session_id),
        format!("TASK_ID={}", spec.session_id),
        format!("TASK_PAYLOAD_JSON={payload_json}"),
        format!("CCTUI_URL={cctui_url}"),
    ];
    if let Some(n) = task_name {
        env.push(format!("TASK_NAME={n}"));
    }
    if let Some(u) = &spec.reply_url {
        env.push(format!("REPLY_URL={u}"));
    }
    Ok(BaseEnv { env, machine_key })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(session_id: &str, payload: serde_json::Value) -> WireDispatchSpec {
        WireDispatchSpec {
            session_id: session_id.to_owned(),
            timeout_minutes: Some(30),
            reply_url: Some("https://cb".to_owned()),
            dedup_key: None,
            profile: None,
            payload,
        }
    }

    #[test]
    fn worker_name_is_deterministic_and_prefixed() {
        let a = worker_name("session-xyz");
        assert_eq!(a, worker_name("session-xyz"));
        assert!(a.starts_with("cctui-worker-"));
        assert_eq!(a.len(), "cctui-worker-".len() + 12);
        assert_ne!(a, worker_name("session-abc"));
    }

    #[test]
    fn dedup_source_prefers_dedup_key_then_session() {
        let mut s = spec("sess-1", json!({}));
        assert_eq!(dedup_source(&s), "sess-1");
        s.dedup_key = Some("logical-key".to_owned());
        assert_eq!(dedup_source(&s), "logical-key");
        s.dedup_key = Some(String::new());
        assert_eq!(dedup_source(&s), "sess-1");
    }

    #[test]
    fn label_safe_sanitizes_and_falls_back() {
        assert_eq!(label_safe("triage:PROJ:2026"), "triage-PROJ-2026");
        assert_eq!(label_safe("a b/c"), "a-b-c");
        assert_eq!(label_safe(":::"), "session");
        assert_eq!(label_safe(""), "session");
        assert_eq!(label_safe(&"x".repeat(100)).len(), 63);
        assert!(label_safe(&"x".repeat(100)).len() <= 63);
    }

    #[test]
    fn build_env_lifts_machine_key_and_keeps_name() {
        let s = spec(
            "sess-9",
            json!({ "name": "Review #7", "cctui_machine_key": "SECRET", "flow": "review" }),
        );
        let base = build_env(&s, "https://cctui.example.test").unwrap();
        assert_eq!(base.machine_key.as_deref(), Some("SECRET"));
        assert!(base.env.contains(&"SESSION_ID=sess-9".to_owned()));
        assert!(base.env.contains(&"TASK_ID=sess-9".to_owned()));
        assert!(base.env.contains(&"TASK_NAME=Review #7".to_owned()));
        assert!(base.env.contains(&"CCTUI_URL=https://cctui.example.test".to_owned()));
        assert!(base.env.contains(&"REPLY_URL=https://cb".to_owned()));
        let tp = base.env.iter().find(|e| e.starts_with("TASK_PAYLOAD_JSON=")).unwrap();
        assert!(!tp.contains("SECRET"), "machine key leaked into payload: {tp}");
        assert!(tp.contains("review"));
        assert!(tp.contains("Review #7"), "name stays in the payload");
    }

    #[test]
    fn build_env_omits_optional_vars_when_absent() {
        let mut s = spec("sess-min", json!({ "flow": "review" }));
        s.reply_url = None;
        let base = build_env(&s, "https://cctui.example.test").unwrap();
        assert!(base.machine_key.is_none(), "no machine key in payload => none lifted");
        assert!(base.env.iter().all(|e| !e.starts_with("REPLY_URL=")), "no reply_url => no var");
        assert!(base.env.iter().all(|e| !e.starts_with("TASK_NAME=")), "no name => no TASK_NAME");
        assert!(base.env.iter().all(|e| !e.contains("cctui_machine_key")));
    }

    #[test]
    fn label_safe_preserves_case_and_trims_edges() {
        assert_eq!(label_safe("-_.MixedCase._-"), "MixedCase");
        assert_eq!(label_safe("UPPER"), "UPPER");
    }
}
