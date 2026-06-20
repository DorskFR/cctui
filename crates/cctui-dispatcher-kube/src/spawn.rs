//! Kubernetes Job spawn mechanics for the standalone kube dispatcher.
//!
//! Lifted from the transitional in-process `cctui-server/src/dispatchers/kube.rs`
//! (CCT-247/248): clones a suspended worker CronJob's `jobTemplate` into a
//! one-shot `batch/v1` Job, injecting per-session env. Runs in-cluster using the
//! pod's projected ServiceAccount token (in-cluster config); the dispatcher's
//! own ServiceAccount carries `cronjobs: get` + `jobs: get,create,delete` in the
//! worker namespace. The server keeps its in-process copy as a transitional
//! shape until CCT-248 parts 2-4 land; this is the enrolled-executor home for
//! the same mechanics.
//!
//! Semantics preserved verbatim from the in-process dispatcher:
//! - Job name = `claude-worker-<sha1(session_id)[:12]>` so a repeat dispatch of
//!   the same session maps to the same Job (CCT-168 collision fix).
//! - 409 on create → read the existing Job: in-flight ⇒ `deduplicated`;
//!   terminal (Complete/Failed) ⇒ delete + recreate ⇒ `redispatched` (CCT-207).
//! - `cctui_machine_key` is lifted out of the payload into `CCTUI_MACHINE_KEY`
//!   and kept OUT of `TASK_PAYLOAD_JSON` (CCT-191).
//! - reply_url → `REPLY_URL` env so the terminal callback (CCT-120) fires.
//!
//! ⚠️ Repo is PUBLIC — no homelab namespaces/images/registries here; the
//! namespace + source CronJob (and thus the worker pod-spec) come from the
//! dispatcher's own config.
#![allow(clippy::doc_markdown)]

use std::time::Duration;

use cctui_proto::ws::WireDispatchSpec;
use k8s_openapi::api::batch::v1::{CronJob, Job};
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DeleteParams, ListParams, PostParams, PropagationPolicy};
use kube::{Client, Error as KubeError};

/// A pod wedged `Pending` longer than this (no schedulable node / image still
/// failing to pull) is reported `Failed` rather than `Running` (CCT-429).
const PENDING_FAILURE_SECS: i64 = 300;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const LABEL_ORIGIN: &str = "cctui.dev/origin";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";
const ANNOTATION_SESSION_ID: &str = "cctui.dev/session-id";

/// Lifecycle state of a spawned Job handle.
#[derive(Debug, Clone, Copy)]
pub enum HandleState {
    Running,
    Complete,
    Failed,
    Gone,
}

impl HandleState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Gone => "gone",
        }
    }
}

/// Outcome of a dispatch: an opaque handle, the namespace, plus the idempotency
/// status reported back to the server verbatim.
#[derive(Debug, Clone)]
pub struct SpawnOutcome {
    pub handle: String,
    pub namespace: String,
    pub status: String,
}

pub struct Spawner {
    namespace: String,
    source_cronjob: String,
    cctui_url: String,
    client: Client,
}

impl Spawner {
    /// Connect using in-cluster config (the pod's projected ServiceAccount
    /// token) — or the local kubeconfig when run off-cluster — so a missing
    /// kube context fails loudly at startup rather than on first dispatch.
    pub async fn connect(
        namespace: String,
        source_cronjob: String,
        cctui_url: String,
    ) -> anyhow::Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self { namespace, source_cronjob, cctui_url, client })
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

    /// Coerce an arbitrary string into a valid k8s label value (≤63 chars,
    /// `[A-Za-z0-9_.-]`, trimmed, stable fallback when empty).
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
    /// a simple by-name upsert.
    fn build_job(
        cctui_url: &str,
        cronjob: &CronJob,
        spec: &WireDispatchSpec,
        name: &str,
    ) -> anyhow::Result<Job> {
        let jt = cronjob
            .spec
            .as_ref()
            .and_then(|s| s.job_template.spec.as_ref())
            .ok_or_else(|| anyhow::anyhow!("source CronJob has no jobTemplate.spec"))?;

        let mut pod_template: Value = serde_json::to_value(&jt.template)?;

        // Lift the machine key out of the payload (CCT-191) so it never lands
        // in TASK_PAYLOAD_JSON, then build the env overrides.
        let mut payload = spec.payload.clone();
        let machine_key = payload
            .as_object_mut()
            .and_then(|o| o.remove("cctui_machine_key"))
            .and_then(|v| v.as_str().map(ToOwned::to_owned));
        let task_name = payload.get("name").and_then(|v| v.as_str()).map(ToOwned::to_owned);
        let payload_json = serde_json::to_string(&payload)?;

        let mut overrides: Vec<(String, String)> = vec![
            ("SESSION_ID".into(), spec.session_id.clone()),
            ("TASK_ID".into(), spec.session_id.clone()),
            ("TASK_PAYLOAD_JSON".into(), payload_json),
            ("CCTUI_URL".into(), cctui_url.to_owned()),
        ];
        if let Some(n) = task_name {
            overrides.push(("TASK_NAME".into(), n));
        }
        if let Some(k) = machine_key {
            overrides.push(("CCTUI_MACHINE_KEY".into(), k));
        }
        if let Some(u) = &spec.reply_url {
            overrides.push(("REPLY_URL".into(), u.clone()));
        }

        let worker = pod_template
            .pointer_mut("/spec/containers/0")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| anyhow::anyhow!("pod template has no containers[0]"))?;

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
        labels[LABEL_SESSION_ID] = json!(Self::label_safe(&spec.session_id));
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
                    LABEL_SESSION_ID: Self::label_safe(&spec.session_id),
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

        Ok(serde_json::from_value(job_json)?)
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
    async fn delete_and_wait(&self, name: &str) -> anyhow::Result<()> {
        let jobs = self.jobs();
        let dp = DeleteParams {
            propagation_policy: Some(PropagationPolicy::Background),
            ..Default::default()
        };
        match jobs.delete(name, &dp).await {
            Ok(_) => {}
            Err(KubeError::Api(e)) if e.code == 404 => {}
            Err(e) => anyhow::bail!("deleting terminal Job: {e}"),
        }
        for _ in 0..60 {
            match jobs.get(name).await {
                Err(KubeError::Api(e)) if e.code == 404 => return Ok(()),
                Err(e) => anyhow::bail!("waiting for Job deletion: {e}"),
                Ok(_) => tokio::time::sleep(Duration::from_millis(500)).await,
            }
        }
        anyhow::bail!("timed out deleting Job {name}")
    }

    async fn create(&self, job: &Job, status: &str, name: &str) -> Result<SpawnOutcome, KubeError> {
        self.jobs().create(&PostParams::default(), job).await?;
        Ok(SpawnOutcome {
            handle: format!("jobs/{name}"),
            namespace: self.namespace.clone(),
            status: status.to_owned(),
        })
    }

    /// Spawn a worker Job for the session. Idempotent: a repeat dispatch of the
    /// same session reuses the deterministic name; a 409 (name in use) is
    /// resolved by reading the existing Job — in-flight ⇒ `deduplicated`,
    /// terminal ⇒ delete + recreate ⇒ `redispatched`.
    pub async fn dispatch(&self, spec: &WireDispatchSpec) -> anyhow::Result<SpawnOutcome> {
        if spec.session_id.is_empty() {
            anyhow::bail!("session_id is required");
        }

        let cronjobs: Api<CronJob> = Api::namespaced(self.client.clone(), &self.namespace);
        let cronjob = cronjobs
            .get(&self.source_cronjob)
            .await
            .map_err(|e| anyhow::anyhow!("reading source CronJob: {e}"))?;

        let name = Self::job_name(&spec.session_id);
        let job = Self::build_job(&self.cctui_url, &cronjob, spec, &name)?;

        match self.create(&job, "dispatched", &name).await {
            Ok(h) => return Ok(h),
            Err(KubeError::Api(e)) if e.code == 409 => {}
            Err(e) => anyhow::bail!("creating Job: {e}"),
        }

        // 409: a prior dispatch of this session already made the Job. Dedup vs.
        // redispatch depends on whether that Job is terminal (CCT-207).
        let existing = match self.jobs().get(&name).await {
            Ok(j) => j,
            // Raced its own teardown — name is free again, create afresh.
            Err(KubeError::Api(e)) if e.code == 404 => {
                return self
                    .create(&job, "dispatched", &name)
                    .await
                    .map_err(|e| anyhow::anyhow!("creating Job: {e}"));
            }
            Err(e) => anyhow::bail!("reading existing Job: {e}"),
        };

        if Self::job_terminal_state(&existing).is_none() {
            // In flight: keep idempotent dedup; the original run fires the callback.
            return Ok(SpawnOutcome {
                handle: format!("jobs/{name}"),
                namespace: self.namespace.clone(),
                status: "deduplicated".to_owned(),
            });
        }

        // Terminal: delete + recreate so a fresh run fires the callback.
        self.delete_and_wait(&name).await?;
        match self.create(&job, "redispatched", &name).await {
            Ok(h) => Ok(h),
            // A concurrent re-dispatch beat us — its run owns the callback now.
            Err(KubeError::Api(e)) if e.code == 409 => Ok(SpawnOutcome {
                handle: format!("jobs/{name}"),
                namespace: self.namespace.clone(),
                status: "deduplicated".to_owned(),
            }),
            Err(e) => anyhow::bail!("recreating Job: {e}"),
        }
    }

    /// Lifecycle of a Job handle, plus a human reason when it FAILED (CCT-429).
    ///
    /// The Job's `conditions` only carry a terminal `Failed` once `backoffLimit`
    /// / `activeDeadlineSeconds` trips — which can be slow or never (an
    /// unschedulable pod sits `Pending` forever). So when the Job isn't terminal
    /// yet we inspect its pods directly and treat a doomed pod state
    /// (CrashLoopBackOff / OOMKilled / image-pull failure / stuck-`Pending`) as
    /// `Failed` with a reason, so the server's death-detector fires promptly
    /// instead of reading the workload as alive until the backoff budget runs.
    pub async fn status(&self, handle: &str) -> anyhow::Result<(HandleState, Option<String>)> {
        let name = handle.strip_prefix("jobs/").unwrap_or(handle);
        let job = match self.jobs().get(name).await {
            Ok(j) => j,
            Err(KubeError::Api(e)) if e.code == 404 => return Ok((HandleState::Gone, None)),
            Err(e) => anyhow::bail!("reading Job {name}: {e}"),
        };
        match Self::job_terminal_state(&job) {
            Some("Complete") => return Ok((HandleState::Complete, None)),
            Some("Failed") => {
                let reason = Self::job_failed_reason(&job);
                return Ok((HandleState::Failed, reason.or(self.pod_failure_reason(name).await)));
            }
            _ => {}
        }
        // Job not terminal — look for a doomed pod the Job condition won't
        // surface until backoff is exhausted.
        match self.pod_failure_reason(name).await {
            Some(reason) => Ok((HandleState::Failed, Some(reason))),
            None => Ok((HandleState::Running, None)),
        }
    }

    fn pods(&self) -> Api<Pod> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    /// The `Failed` Job condition's reason/message, if present.
    fn job_failed_reason(job: &Job) -> Option<String> {
        let conditions = job.status.as_ref().and_then(|s| s.conditions.as_ref())?;
        conditions.iter().find(|c| c.type_ == "Failed" && c.status == "True").map(|c| {
            c.message
                .clone()
                .or_else(|| c.reason.clone())
                .unwrap_or_else(|| "job failed".to_owned())
        })
    }

    /// Inspect the Job's pods for a terminal-but-not-yet-Job-Failed condition
    /// (CCT-429): a crash-looping / OOMKilled / un-pullable container, or a pod
    /// wedged `Pending` past [`PENDING_FAILURE_SECS`]. Returns a reason string
    /// when one is found, else `None` (still legitimately running/starting). A
    /// listing error degrades to `None` so a transient API hiccup never
    /// misreports a live workload as dead.
    async fn pod_failure_reason(&self, job_name: &str) -> Option<String> {
        let lp = ListParams::default().labels(&format!("job-name={job_name}"));
        let pods = self.pods().list(&lp).await.ok()?;
        for pod in pods {
            let status = pod.status.as_ref()?;
            // Any container (init or main) stuck waiting on a doomed reason, or
            // terminated by OOM, is fatal for this pod.
            let waiting_or_oom = status
                .init_container_statuses
                .iter()
                .chain(status.container_statuses.iter())
                .flatten()
                .find_map(|cs| {
                    let st = cs.state.as_ref()?;
                    if let Some(w) = &st.waiting {
                        if let Some(r) = &w.reason {
                            if matches!(
                                r.as_str(),
                                "CrashLoopBackOff"
                                    | "ImagePullBackOff"
                                    | "ErrImagePull"
                                    | "CreateContainerError"
                                    | "CreateContainerConfigError"
                                    | "InvalidImageName"
                            ) {
                                return Some(r.clone());
                            }
                        }
                    }
                    if let Some(t) = &st.terminated {
                        if t.reason.as_deref() == Some("OOMKilled") {
                            return Some("OOMKilled".to_owned());
                        }
                    }
                    None
                });
            if let Some(reason) = waiting_or_oom {
                return Some(reason);
            }
            // Pod wedged Pending (unschedulable / image still pulling) past the
            // grace window: treat as failed so an unschedulable Job doesn't read
            // as alive forever.
            if status.phase.as_deref() == Some("Pending") {
                if let Some(created) = pod.metadata.creation_timestamp.as_ref() {
                    let age = chrono::Utc::now().signed_duration_since(created.0).num_seconds();
                    if age >= PENDING_FAILURE_SECS {
                        return Some(format!("pod pending for {age}s (unschedulable?)"));
                    }
                }
            }
            if status.phase.as_deref() == Some("Failed") {
                return Some(status.reason.clone().unwrap_or_else(|| "pod failed".to_owned()));
            }
        }
        None
    }

    pub async fn cancel(&self, handle: &str) -> anyhow::Result<()> {
        let name = handle.strip_prefix("jobs/").unwrap_or(handle);
        self.delete_and_wait(name).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(session_id: &str, payload: Value) -> WireDispatchSpec {
        WireDispatchSpec {
            session_id: session_id.to_owned(),
            timeout_minutes: Some(30),
            reply_url: Some("https://cb".to_owned()),
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
        let a = Spawner::job_name("triage:PROJ:2026060116");
        let b = Spawner::job_name("triage:PROJ:2026060116");
        assert_eq!(a, b);
        assert!(a.starts_with("claude-worker-"));
        assert!(a.len() <= 63);
        assert!(a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert_ne!(a, Spawner::job_name("triage:PROJ:2026060117"));
    }

    #[test]
    fn label_safe_sanitizes_separators() {
        assert_eq!(Spawner::label_safe("triage:PROJ:2026"), "triage-PROJ-2026");
        assert_eq!(Spawner::label_safe(":::"), "session");
        assert_eq!(Spawner::label_safe(&"x".repeat(100)).len(), 63);
    }

    #[test]
    fn build_job_injects_env_and_drops_machine_key_from_payload() {
        let payload =
            json!({ "flow": "review", "name": "Review #5697", "cctui_machine_key": "SECRET" });
        let s = spec("sess-123", payload);
        let name = Spawner::job_name("sess-123");
        let job = Spawner::build_job(
            "http://cctui.example.svc.cluster.local:8700",
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
