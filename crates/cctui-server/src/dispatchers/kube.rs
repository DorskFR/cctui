//! In-process Kubernetes dispatcher (CCT-234).
//!
//! TRANSITIONAL — to be removed (CCT-248). The corrected model makes the server
//! an orchestrator that never touches the kube API (and carries no k8s RBAC):
//! these Job mechanics move into a standalone, per-account *enrolled*
//! `cctui-dispatcher-kube` executor (CCT-247) that the server reaches over the
//! wire, then this module is deleted. Kept here only until that executor lands
//! and soaks.
//!
//! Clones a suspended worker CronJob's `jobTemplate` into a one-shot
//! `batch/v1` Job, injecting the per-session env. Runs inside cctui-server
//! using the pod's projected ServiceAccount token (in-cluster config); the
//! worker namespace and its RBAC (`cronjobs: get` + `jobs: get,create,delete`)
//! are provisioned out-of-band in your cluster manifests.
//!
//! Semantics preserved verbatim from the python service:
//! - Job name = `claude-worker-<sha1(session_id)[:12]>` so a repeat dispatch of
//!   the same session maps to the same Job (CCT-168 collision fix).
//! - 409 on create → read the existing Job: in-flight ⇒ `deduplicated`;
//!   terminal (Complete/Failed) ⇒ delete + recreate ⇒ `redispatched` (CCT-207).
//! - `cctui_machine_key` is lifted out of the payload into `CCTUI_MACHINE_KEY`
//!   and kept OUT of `TASK_PAYLOAD_JSON` (CCT-191).
//! - reply_url → `REPLY_URL` env so the terminal callback (CCT-120) fires.
#![allow(clippy::doc_markdown)]

use std::time::Duration;

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use kube::Client;
use kube::api::{Api, DeleteParams, PostParams, PropagationPolicy};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher, HandleStatus};

const LABEL_ORIGIN: &str = "cctui.dev/origin";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";
const ANNOTATION_SESSION_ID: &str = "cctui.dev/session-id";

/// Config for one KubeDispatcher instance, parsed from `CCTUI_DISPATCHERS`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct KubeDispatcherConfig {
    pub id: String,
    /// Namespace the worker Job + source CronJob live in (e.g. `workers`).
    pub namespace: String,
    /// Suspended CronJob whose `jobTemplate` is cloned (e.g. `worker-template`).
    pub source_cronjob: String,
    /// `CCTUI_URL` injected into the worker so its daemon dials back.
    #[serde(default)]
    pub cctui_url: Option<String>,
}

pub struct KubeDispatcher {
    id: String,
    namespace: String,
    source_cronjob: String,
    cctui_url: Option<String>,
    client: Client,
}

impl KubeDispatcher {
    /// Build from in-cluster config. Returns `None` (logged by the caller) when
    /// no kube config is reachable — keeps the dispatcher unregistered off-cluster.
    pub async fn try_new(cfg: &KubeDispatcherConfig) -> anyhow::Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self {
            id: cfg.id.clone(),
            namespace: cfg.namespace.clone(),
            source_cronjob: cfg.source_cronjob.clone(),
            cctui_url: cfg.cctui_url.clone(),
            client,
        })
    }

    fn jobs(&self) -> Api<Job> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    /// `claude-worker-<sha1(session_id)[:12]>` — deterministic so a repeat
    /// dispatch maps to the same Job (idempotency key, CCT-168/207).
    fn job_name(session_id: &str) -> String {
        let digest = Sha1::digest(session_id.as_bytes());
        let hex = hex::encode(digest);
        format!("claude-worker-{}", &hex[..12])
    }

    /// Public alias for [`Self::label_safe`] so the docker dispatcher can reuse
    /// the same sanitization for its `cctui.dev/session-id` label.
    pub fn label_safe_for(value: &str) -> String {
        Self::label_safe(value)
    }

    /// Coerce an arbitrary string into a valid k8s label value (≤63 chars,
    /// `[A-Za-z0-9_.-]`). Mirrors the python `label_safe`.
    fn label_safe(value: &str) -> String {
        let mapped: String = value
            .chars()
            .map(
                |c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-') { c } else { '-' },
            )
            .collect();
        let truncated: String = mapped.chars().take(63).collect();
        let trimmed = truncated.trim_matches(|c| matches!(c, '-' | '_' | '.'));
        if trimmed.is_empty() { "session".to_owned() } else { trimmed.to_owned() }
    }

    /// Build the Job JSON from the source CronJob's jobTemplate + per-session
    /// overrides. Manipulates the pod template as serde_json so env merge stays
    /// a simple by-name upsert (mirrors the python `merge_env`).
    fn build_job(
        cctui_url: Option<&str>,
        cronjob: &CronJob,
        spec: &DispatchSpec<'_>,
        name: &str,
    ) -> Result<Job, DispatchError> {
        let jt =
            cronjob.spec.as_ref().and_then(|s| s.job_template.spec.as_ref()).ok_or_else(|| {
                DispatchError::Backend("source CronJob has no jobTemplate.spec".into())
            })?;

        let mut pod_template: Value = serde_json::to_value(&jt.template)
            .map_err(|e| DispatchError::Backend(format!("serializing pod template: {e}")))?;

        // Lift the machine key out of the payload (CCT-191) so it never lands
        // in TASK_PAYLOAD_JSON, then build the env overrides.
        let mut payload = spec.payload.clone();
        let machine_key = payload
            .as_object_mut()
            .and_then(|o| o.remove("cctui_machine_key"))
            .and_then(|v| v.as_str().map(ToOwned::to_owned));
        let task_name = payload.get("name").and_then(|v| v.as_str()).map(ToOwned::to_owned);
        let payload_json = serde_json::to_string(&payload)
            .map_err(|e| DispatchError::Backend(format!("serializing payload: {e}")))?;

        let mut overrides: Vec<(String, String)> = vec![
            ("SESSION_ID".into(), spec.session_id.to_owned()),
            ("TASK_ID".into(), spec.session_id.to_owned()),
            ("TASK_PAYLOAD_JSON".into(), payload_json),
        ];
        if let Some(url) = cctui_url {
            overrides.push(("CCTUI_URL".into(), url.to_owned()));
        }
        if let Some(n) = task_name {
            overrides.push(("TASK_NAME".into(), n));
        }
        if let Some(k) = machine_key {
            overrides.push(("CCTUI_MACHINE_KEY".into(), k));
        }
        if let Some(u) = spec.reply_url {
            overrides.push(("REPLY_URL".into(), u.to_owned()));
        }

        let worker = pod_template
            .pointer_mut("/spec/containers/0")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| DispatchError::Backend("pod template has no containers[0]".into()))?;

        // Drop the CronJob's `sleep infinity` debug override so the real
        // ENTRYPOINT runs.
        worker.remove("command");
        worker.remove("args");
        Self::merge_env(worker, &overrides);

        // Stamp origin/session labels + the full-id annotation.
        let meta =
            pod_template.as_object_mut().unwrap().entry("metadata").or_insert_with(|| json!({}));
        let meta = meta.as_object_mut().unwrap();
        let labels = meta.entry("labels").or_insert_with(|| json!({}));
        labels[LABEL_ORIGIN] = json!("cctui-kube-dispatcher");
        labels[LABEL_SESSION_ID] = json!(Self::label_safe(spec.session_id));
        let annotations = meta.entry("annotations").or_insert_with(|| json!({}));
        annotations[ANNOTATION_SESSION_ID] = json!(spec.session_id);

        let deadline =
            spec.timeout_minutes.map(|m| i64::from(m) * 60).or(jt.active_deadline_seconds);

        let job_json = json!({
            "apiVersion": "batch/v1",
            "kind": "Job",
            "metadata": {
                "name": name,
                "labels": {
                    LABEL_ORIGIN: "cctui-kube-dispatcher",
                    LABEL_SESSION_ID: Self::label_safe(spec.session_id),
                },
                "annotations": { ANNOTATION_SESSION_ID: spec.session_id },
            },
            "spec": {
                "backoffLimit": 0,
                "ttlSecondsAfterFinished": 86400,
                "activeDeadlineSeconds": deadline,
                "template": pod_template,
            },
        });

        serde_json::from_value(job_json)
            .map_err(|e| DispatchError::Backend(format!("building Job object: {e}")))
    }

    /// Upsert env vars by name into the container's `env` array, preserving the
    /// template's existing entries (including `valueFrom`).
    fn merge_env(worker: &mut serde_json::Map<String, Value>, overrides: &[(String, String)]) {
        let env = worker.entry("env").or_insert_with(|| json!([]));
        let arr = env.as_array_mut().expect("env is an array");
        for (k, v) in overrides {
            if let Some(existing) =
                arr.iter_mut().find(|e| e.get("name").and_then(Value::as_str) == Some(k.as_str()))
            {
                let obj = existing.as_object_mut().unwrap();
                obj.insert("value".into(), json!(v));
                obj.remove("valueFrom");
            } else {
                arr.push(json!({ "name": k, "value": v }));
            }
        }
    }

    /// 'Complete' / 'Failed' if the Job carries a terminal condition, else None.
    fn job_terminal_state(job: &Job) -> Option<&'static str> {
        let conditions = job.status.as_ref().and_then(|s| s.conditions.as_ref());
        for cond in conditions.into_iter().flatten() {
            if cond.status == "True" && matches!(cond.type_.as_str(), "Complete" | "Failed") {
                return Some(if cond.type_ == "Complete" { "Complete" } else { "Failed" });
            }
        }
        None
    }

    /// Delete a Job and block until its name is free (background propagation
    /// GCs pods async; poll until the read 404s so the recreate can't race).
    async fn delete_and_wait(&self, name: &str) -> Result<(), DispatchError> {
        let jobs = self.jobs();
        let dp = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Background),
            ..Default::default()
        };
        match jobs.delete(name, &dp).await {
            Ok(_) => {}
            Err(kube::Error::Api(e)) if e.code == 404 => {}
            Err(e) => return Err(DispatchError::Backend(format!("deleting terminal Job: {e}"))),
        }
        for _ in 0..60 {
            match jobs.get(name).await {
                Err(kube::Error::Api(e)) if e.code == 404 => return Ok(()),
                Err(e) => {
                    return Err(DispatchError::Backend(format!("waiting for Job deletion: {e}")));
                }
                Ok(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
        Err(DispatchError::Backend(format!("timed out deleting Job {name}")))
    }

    async fn create(
        &self,
        job: &Job,
        status: &str,
        name: &str,
    ) -> Result<DispatchHandle, kube::Error> {
        self.jobs().create(&PostParams::default(), job).await?;
        Ok(DispatchHandle {
            handle: format!("jobs/{name}"),
            namespace: Some(self.namespace.clone()),
            status: Some(status.to_owned()),
        })
    }
}

#[async_trait]
impl Dispatcher for KubeDispatcher {
    fn id(&self) -> &str {
        &self.id
    }

    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError> {
        if spec.session_id.is_empty() {
            return Err(DispatchError::InvalidIntent("session_id is required".into()));
        }

        let cronjobs: Api<CronJob> = Api::namespaced(self.client.clone(), &self.namespace);
        let cronjob = cronjobs
            .get(&self.source_cronjob)
            .await
            .map_err(|e| DispatchError::Backend(format!("reading source CronJob: {e}")))?;

        let name = Self::job_name(spec.session_id);
        let job = Self::build_job(self.cctui_url.as_deref(), &cronjob, spec, &name)?;

        match self.create(&job, "dispatched", &name).await {
            Ok(h) => return Ok(h),
            Err(kube::Error::Api(e)) if e.code == 409 => {}
            Err(e) => return Err(DispatchError::Backend(format!("creating Job: {e}"))),
        }

        // 409: a prior dispatch of this session already made the Job. Dedup vs.
        // redispatch depends on whether that Job is terminal (CCT-207).
        let existing = match self.jobs().get(&name).await {
            Ok(j) => j,
            // Raced its own teardown — name is free again, create afresh.
            Err(kube::Error::Api(e)) if e.code == 404 => {
                return self
                    .create(&job, "dispatched", &name)
                    .await
                    .map_err(|e| DispatchError::Backend(format!("creating Job: {e}")));
            }
            Err(e) => return Err(DispatchError::Backend(format!("reading existing Job: {e}"))),
        };

        if Self::job_terminal_state(&existing).is_none() {
            // In flight: keep idempotent dedup; the original run fires the callback.
            return Ok(DispatchHandle {
                handle: format!("jobs/{name}"),
                namespace: Some(self.namespace.clone()),
                status: Some("deduplicated".into()),
            });
        }

        // Terminal: delete + recreate so a fresh run fires the callback.
        self.delete_and_wait(&name).await?;
        match self.create(&job, "redispatched", &name).await {
            Ok(h) => Ok(h),
            // A concurrent re-dispatch beat us — its run owns the callback now.
            Err(kube::Error::Api(e)) if e.code == 409 => Ok(DispatchHandle {
                handle: format!("jobs/{name}"),
                namespace: Some(self.namespace.clone()),
                status: Some("deduplicated".into()),
            }),
            Err(e) => Err(DispatchError::Backend(format!("recreating Job: {e}"))),
        }
    }

    async fn status(&self, handle: &str) -> Result<HandleStatus, DispatchError> {
        let name = handle.strip_prefix("jobs/").unwrap_or(handle);
        match self.jobs().get(name).await {
            Ok(job) => Ok(match Self::job_terminal_state(&job) {
                Some("Complete") => HandleStatus::Complete,
                Some("Failed") => HandleStatus::Failed(None),
                _ => HandleStatus::Running,
            }),
            Err(kube::Error::Api(e)) if e.code == 404 => Ok(HandleStatus::Gone),
            Err(e) => Err(DispatchError::Backend(format!("reading Job {name}: {e}"))),
        }
    }

    async fn cancel(&self, handle: &str) -> Result<(), DispatchError> {
        let name = handle.strip_prefix("jobs/").unwrap_or(handle);
        self.delete_and_wait(name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec<'a>(session_id: &'a str, payload: &'a Value) -> DispatchSpec<'a> {
        DispatchSpec {
            session_id,
            timeout_minutes: Some(30),
            reply_url: Some("https://cb"),
            payload,
        }
    }

    fn sample_cronjob() -> CronJob {
        serde_json::from_value(json!({
            "apiVersion": "batch/v1",
            "kind": "CronJob",
            "metadata": { "name": "worker-template" },
            "spec": {
                "schedule": "0 0 31 2 *",
                "jobTemplate": {
                    "spec": {
                        "activeDeadlineSeconds": 86400,
                        "template": {
                            "metadata": { "labels": { "app.kubernetes.io/name": "claude-worker" } },
                            "spec": {
                                "restartPolicy": "Never",
                                "containers": [{
                                    "name": "worker",
                                    "image": "example.com/worker:latest",
                                    "command": ["sleep", "infinity"],
                                    "env": [
                                        { "name": "TASK_ID", "value": "" },
                                        { "name": "POD_NAME", "valueFrom": { "fieldRef": { "fieldPath": "metadata.name" } } }
                                    ]
                                }]
                            }
                        }
                    }
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn job_name_is_deterministic_and_dns_safe() {
        let a = KubeDispatcher::job_name("triage:PROJ:2026060116");
        let b = KubeDispatcher::job_name("triage:PROJ:2026060116");
        assert_eq!(a, b);
        assert!(a.starts_with("claude-worker-"));
        assert!(a.len() <= 63);
        assert!(a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert_ne!(a, KubeDispatcher::job_name("triage:PROJ:2026060117"));
    }

    #[test]
    fn label_safe_sanitizes_separators() {
        assert_eq!(KubeDispatcher::label_safe("triage:PROJ:2026"), "triage-PROJ-2026");
        assert_eq!(KubeDispatcher::label_safe(":::"), "session");
        assert_eq!(KubeDispatcher::label_safe(&"x".repeat(100)).len(), 63);
    }

    #[test]
    fn build_job_injects_env_and_drops_machine_key_from_payload() {
        let payload =
            json!({ "flow": "review", "name": "Review #5697", "cctui_machine_key": "SECRET" });
        let s = spec("sess-123", &payload);
        let name = KubeDispatcher::job_name("sess-123");
        let job = KubeDispatcher::build_job(
            Some("http://cctui.dev.svc.cluster.local:8700"),
            &sample_cronjob(),
            &s,
            &name,
        )
        .unwrap();
        let v = serde_json::to_value(&job).unwrap();

        let env = v.pointer("/spec/template/spec/containers/0/env").unwrap().as_array().unwrap();
        let get = |k: &str| {
            env.iter()
                .find(|e| e["name"] == k)
                .and_then(|e| e.get("value"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
        };
        assert_eq!(get("SESSION_ID").as_deref(), Some("sess-123"));
        assert_eq!(get("TASK_ID").as_deref(), Some("sess-123"));
        assert_eq!(get("TASK_NAME").as_deref(), Some("Review #5697"));
        assert_eq!(get("CCTUI_MACHINE_KEY").as_deref(), Some("SECRET"));
        assert_eq!(get("REPLY_URL").as_deref(), Some("https://cb"));
        // Machine key must NOT appear inside TASK_PAYLOAD_JSON.
        let tp = get("TASK_PAYLOAD_JSON").unwrap();
        assert!(!tp.contains("SECRET"), "machine key leaked into payload: {tp}");
        assert!(tp.contains("review"));
        // POD_NAME's valueFrom is preserved (not clobbered into a value).
        let pod_name = env.iter().find(|e| e["name"] == "POD_NAME").unwrap();
        assert!(pod_name.get("valueFrom").is_some());
        // sleep-infinity debug override dropped so the real entrypoint runs.
        assert!(v.pointer("/spec/template/spec/containers/0/command").is_none());
        // Deterministic deadline from timeout_minutes (30 * 60).
        assert_eq!(v.pointer("/spec/activeDeadlineSeconds"), Some(&json!(1800)));
        // Origin + session labels stamped on the Job.
        assert_eq!(
            v.pointer("/metadata/labels/cctui.dev~1origin"),
            Some(&json!("cctui-kube-dispatcher"))
        );
        assert_eq!(
            v.pointer("/metadata/annotations/cctui.dev~1session-id"),
            Some(&json!("sess-123"))
        );
    }
}
