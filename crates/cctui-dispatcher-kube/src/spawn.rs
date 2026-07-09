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
//! - Job name = `cctui-worker-<sha1(session_id)[:12]>` so a repeat dispatch of
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

/// Auto-reap a finished worker Job (`Complete`/`Failed`) this many seconds after
/// it stops, via `spec.ttlSecondsAfterFinished`. Long enough to inspect a
/// just-finished run, short enough to keep the namespace free of corpses
/// (CCT-518; was 86400 = 24h).
const JOB_TTL_SECONDS: i64 = 3600;

/// Default worker lifetime when the dispatch carries no `timeout_minutes`
/// (CCT-513): 1h, instead of inheriting the source CronJob template's 24h
/// `activeDeadlineSeconds` backstop — a dispatched session that finishes but
/// never signals done should cost at most an hour, not a day. Overridable via
/// `CCTUI_WORKER_DEFAULT_TIMEOUT_MINUTES`; a per-dispatch `timeout_minutes`
/// always wins.
const DEFAULT_TIMEOUT_MINUTES: u32 = 60;
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
    /// Ceiling on concurrently in-flight worker Jobs (CCT-522). `0` ⇒ unlimited.
    max_inflight: usize,
    /// `activeDeadlineSeconds` applied when a dispatch has no
    /// `timeout_minutes` (CCT-513).
    default_deadline_secs: i64,
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
        // Optional concurrency ceiling (CCT-522): `CCTUI_WORKER_MAX_INFLIGHT`,
        // `0`/unset ⇒ unlimited. An explicit cap rejects a dispatch once that
        // many non-terminal dispatcher-owned Jobs already exist, so a webhook
        // flood of DISTINCT keys can't exhaust the cluster — the throttle the
        // old session-id collision used to provide implicitly.
        let max_inflight = std::env::var("CCTUI_WORKER_MAX_INFLIGHT")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let default_deadline_secs = Self::default_deadline_secs(
            std::env::var("CCTUI_WORKER_DEFAULT_TIMEOUT_MINUTES").ok().as_deref(),
        );
        Ok(Self {
            namespace,
            source_cronjob,
            cctui_url,
            client,
            max_inflight,
            default_deadline_secs,
        })
    }

    /// Resolve the no-`timeout_minutes` deadline from
    /// `CCTUI_WORKER_DEFAULT_TIMEOUT_MINUTES` (minutes; unset/unparsable/0 ⇒
    /// [`DEFAULT_TIMEOUT_MINUTES`]), in seconds.
    fn default_deadline_secs(var: Option<&str>) -> i64 {
        let minutes = var
            .and_then(|v| v.trim().parse::<u32>().ok())
            .filter(|m| *m > 0)
            .unwrap_or(DEFAULT_TIMEOUT_MINUTES);
        i64::from(minutes) * 60
    }

    fn jobs(&self) -> Api<Job> {
        Api::namespaced(self.client.clone(), &self.namespace)
    }

    /// `cctui-worker-<sha1(session_id)[:12]>` — deterministic so a repeat
    /// dispatch maps to the same Job (idempotency key, CCT-168/207). The prefix
    /// is `cctui-worker-` (the legacy `claude-worker-` name was renamed under the
    /// cctui unification, CCT-452); derived solely here, so dedup/status/delete
    /// all stay consistent.
    fn job_name(session_id: &str) -> String {
        let digest = Sha1::digest(session_id.as_bytes());
        let hex = hex::encode(digest);
        format!("cctui-worker-{}", &hex[..12])
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
        default_deadline_secs: i64,
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
        // Lift `payload.env` so it becomes real POD env, not a blob buried in
        // TASK_PAYLOAD_JSON. This is what lets the secret surface work: a
        // `vault:…` value is emitted as a literal env var the in-cluster
        // vault-env webhook resolves at exec (before the entrypoint), and a
        // `k8s:[ns/]secret#key` value becomes a `secretKeyRef` the kubelet
        // injects — neither the dispatcher nor the pod spec ever holds the
        // resolved secret. Removing it from the payload also stops the daemon
        // from re-applying the unresolved reference over the resolved env.
        let env_map = payload.as_object_mut().and_then(|o| o.remove("env"));
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

        // Split the lifted env into literal vars (vault:/plain) and k8s
        // secret-key references. Keys are upserted, so an explicit task env wins
        // over a same-named template default.
        let mut secret_refs: Vec<(String, String, String)> = Vec::new();
        if let Some(Value::Object(m)) = env_map {
            for (k, v) in m {
                let Some(val) = v.as_str() else { continue };
                if let Some(rest) = val.strip_prefix("k8s:") {
                    // k8s:[namespace/]secret#key — secretKeyRef is namespace-local
                    // (the pod's own ns), so any namespace prefix is informational
                    // and dropped here.
                    if let Some((left, key)) = rest.split_once('#') {
                        let secret = left.rsplit('/').next().unwrap_or(left);
                        if !secret.is_empty() && !key.is_empty() {
                            secret_refs.push((k, secret.to_owned(), key.to_owned()));
                            continue;
                        }
                    }
                    // Malformed k8s: ref — skip rather than leak the ref string.
                    continue;
                }
                // vault:… (resolved by the vault-env webhook) or a plain literal.
                overrides.push((k, val.to_owned()));
            }
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
        Self::merge_env_secret_refs(worker, &secret_refs);

        // Stamp origin/session labels + the full-id annotation.
        let meta =
            pod_template.as_object_mut().unwrap().entry("metadata").or_insert_with(|| json!({}));
        let meta = meta.as_object_mut().unwrap();
        let labels = meta.entry("labels").or_insert_with(|| json!({}));
        labels[LABEL_ORIGIN] = json!("cctui-kube-dispatcher");
        labels[LABEL_SESSION_ID] = json!(Self::label_safe(&spec.session_id));
        let annotations = meta.entry("annotations").or_insert_with(|| json!({}));
        annotations[ANNOTATION_SESSION_ID] = json!(spec.session_id);

        // Deliberately NOT falling back to the template's
        // `activeDeadlineSeconds` (the infra CronJob's 24h outer backstop):
        // a dispatch without an explicit timeout gets the dispatcher's 1h
        // default instead (CCT-513).
        let deadline = spec.timeout_minutes.map_or(default_deadline_secs, |m| i64::from(m) * 60);

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
                // Set on our built spec (not pulled from the source CronJob's
                // jobTemplate), so this TTL always wins (CCT-518).
                "ttlSecondsAfterFinished": JOB_TTL_SECONDS,
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

    /// Upsert `valueFrom.secretKeyRef` env vars (from `k8s:` references) into the
    /// container's `env`. Points at a secret that must already exist in the
    /// pod's namespace — the kubelet injects the value, so it never enters the
    /// Job spec or the dispatcher process.
    fn merge_env_secret_refs(
        worker: &mut serde_json::Map<String, Value>,
        refs: &[(String, String, String)],
    ) {
        let env = worker.entry("env").or_insert_with(|| json!([]));
        let arr = env.as_array_mut().expect("env is an array");
        for (name, secret, key) in refs {
            let entry = json!({
                "name": name,
                "valueFrom": { "secretKeyRef": { "name": secret, "key": key } },
            });
            if let Some(existing) = arr
                .iter_mut()
                .find(|e| e.get("name").and_then(Value::as_str) == Some(name.as_str()))
            {
                *existing = entry;
            } else {
                arr.push(entry);
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

    /// The string the Job name derives from: the caller's `dedup_key` (the
    /// logical request id) when present, else the `session_id` (CCT-522). Keeping
    /// idempotency on the dedup key lets `session_id` be fresh per dispatch (no
    /// conversation chaining) while a repeat of the same logical key still
    /// coalesces onto one Job.
    fn dedup_source(spec: &WireDispatchSpec) -> &str {
        spec.dedup_key.as_deref().filter(|k| !k.is_empty()).unwrap_or(&spec.session_id)
    }

    /// Spawn a worker Job for the session. Idempotent: a repeat dispatch of the
    /// same dedup key reuses the deterministic name; a 409 (name in use) is
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

        let name = Self::job_name(Self::dedup_source(spec));
        self.enforce_inflight_cap(&name).await?;
        let job =
            Self::build_job(&self.cctui_url, &cronjob, spec, &name, self.default_deadline_secs)?;

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

    /// Reject a dispatch when `max_inflight` non-terminal dispatcher-owned Jobs
    /// already exist (CCT-522). `0` ⇒ unlimited (the default; no behavior
    /// change). The Job we're about to create (`this_name`) is excluded from the
    /// count so a dedup/redispatch of an already-running Job is never blocked by
    /// its own presence. Only consulted when a cap is explicitly configured.
    async fn enforce_inflight_cap(&self, this_name: &str) -> anyhow::Result<()> {
        if self.max_inflight == 0 {
            return Ok(());
        }
        let lp = ListParams::default().labels(&format!("{LABEL_ORIGIN}=cctui-kube-dispatcher"));
        let jobs = self
            .jobs()
            .list(&lp)
            .await
            .map_err(|e| anyhow::anyhow!("listing worker Jobs for concurrency cap: {e}"))?;
        let inflight = jobs
            .items
            .iter()
            .filter(|j| j.metadata.name.as_deref() != Some(this_name))
            .filter(|j| Self::job_terminal_state(j).is_none())
            .count();
        if inflight >= self.max_inflight {
            anyhow::bail!(
                "worker concurrency cap reached: {inflight} in-flight \
                 >= CCTUI_WORKER_MAX_INFLIGHT={}",
                self.max_inflight
            );
        }
        Ok(())
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
        Ok(self
            .pod_failure_reason(name)
            .await
            .map_or((HandleState::Running, None), |reason| (HandleState::Failed, Some(reason))))
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
                    if let Some(w) = &st.waiting
                        && let Some(r) = &w.reason
                        && matches!(
                            r.as_str(),
                            "CrashLoopBackOff"
                                | "ImagePullBackOff"
                                | "ErrImagePull"
                                | "CreateContainerError"
                                | "CreateContainerConfigError"
                                | "InvalidImageName"
                        )
                    {
                        return Some(r.clone());
                    }
                    if let Some(t) = &st.terminated
                        && t.reason.as_deref() == Some("OOMKilled")
                    {
                        return Some("OOMKilled".to_owned());
                    }
                    None
                });
            if let Some(reason) = waiting_or_oom {
                return Some(reason);
            }
            // Pod wedged Pending (unschedulable / image still pulling) past the
            // grace window: treat as failed so an unschedulable Job doesn't read
            // as alive forever.
            if status.phase.as_deref() == Some("Pending")
                && let Some(created) = pod.metadata.creation_timestamp.as_ref()
            {
                let age = chrono::Utc::now().signed_duration_since(created.0).num_seconds();
                if age >= PENDING_FAILURE_SECS {
                    return Some(format!("pod pending for {age}s (unschedulable?)"));
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
            dedup_key: None,
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
        assert!(a.starts_with("cctui-worker-"));
        assert!(a.len() <= 63);
        assert!(a.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert_ne!(a, Spawner::job_name("triage:PROJ:2026060117"));
    }

    #[test]
    fn job_name_derives_from_dedup_key_so_session_id_can_be_fresh() {
        // CCT-522: two dispatches of the same logical key carry DIFFERENT fresh
        // session ids but the SAME dedup key, and must map to the same Job so a
        // duplicate webhook coalesces.
        let mut s1 = spec("11111111-1111-4111-8111-111111111111", json!({}));
        s1.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        let mut s2 = spec("22222222-2222-4222-8222-222222222222", json!({}));
        s2.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        assert_eq!(
            Spawner::job_name(Spawner::dedup_source(&s1)),
            Spawner::job_name(Spawner::dedup_source(&s2)),
            "same dedup key ⇒ same Job despite distinct session ids",
        );
        // With no dedup key the Job name falls back to the (unique) session id.
        let s3 = spec("33333333-3333-4333-8333-333333333333", json!({}));
        assert_eq!(Spawner::dedup_source(&s3), "33333333-3333-4333-8333-333333333333");
        assert_ne!(
            Spawner::job_name(Spawner::dedup_source(&s1)),
            Spawner::job_name(Spawner::dedup_source(&s3)),
            "different keys ⇒ different Jobs (own pod + own session)",
        );
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
            3600,
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

    #[test]
    fn no_timeout_uses_default_deadline_not_template_24h() {
        // CCT-513: a dispatch without `timeout_minutes` must NOT inherit the
        // CronJob template's 86400s backstop — it gets the dispatcher default.
        let mut s = spec("sess-none", json!({}));
        s.timeout_minutes = None;
        let name = Spawner::job_name("sess-none");
        let job =
            Spawner::build_job("http://cctui:8700", &sample_cronjob(), &s, &name, 3600).unwrap();
        let v = serde_json::to_value(&job).unwrap();
        assert_eq!(v.pointer("/spec/activeDeadlineSeconds"), Some(&json!(3600)));
    }

    #[test]
    fn per_dispatch_timeout_overrides_default_deadline() {
        // timeout_minutes=30 wins even when the configured default is larger.
        let s = spec("sess-30", json!({}));
        let name = Spawner::job_name("sess-30");
        let job =
            Spawner::build_job("http://cctui:8700", &sample_cronjob(), &s, &name, 7200).unwrap();
        let v = serde_json::to_value(&job).unwrap();
        assert_eq!(v.pointer("/spec/activeDeadlineSeconds"), Some(&json!(1800)));
    }

    #[test]
    fn default_deadline_env_parsing() {
        // Unset ⇒ 60 min; explicit minutes override; garbage/zero fall back.
        assert_eq!(Spawner::default_deadline_secs(None), 3600);
        assert_eq!(Spawner::default_deadline_secs(Some("30")), 1800);
        assert_eq!(Spawner::default_deadline_secs(Some(" 120 ")), 7200);
        assert_eq!(Spawner::default_deadline_secs(Some("nope")), 3600);
        assert_eq!(Spawner::default_deadline_secs(Some("0")), 3600);
    }

    #[test]
    fn build_job_promotes_payload_env_to_pod_env() {
        let payload = json!({
            "flow": "pr-review",
            "env": {
                "CONTEXT_PACK_URL": "https://github.com/acme/pack",
                "GITHUB_TOKEN": "vault:secret/data/ci#github",
                "SLACK_TOKEN": "k8s:dev/scli-secret#token",
                "YOUTRACK_TOKEN": "k8s:yt-secret#token"
            }
        });
        let s = spec("sess-9", payload);
        let name = Spawner::job_name("sess-9");
        let job =
            Spawner::build_job("http://cctui:8700", &sample_cronjob(), &s, &name, 3600).unwrap();
        let v = serde_json::to_value(&job).unwrap();
        let env = v.pointer("/spec/template/spec/containers/0/env").unwrap().as_array().unwrap();
        let entry = |k: &str| env.iter().find(|e| e["name"] == k).cloned();

        // Plain + vault: values become literal env (vault-env resolves vault: at exec).
        assert_eq!(
            entry("CONTEXT_PACK_URL").unwrap()["value"],
            json!("https://github.com/acme/pack")
        );
        assert_eq!(entry("GITHUB_TOKEN").unwrap()["value"], json!("vault:secret/data/ci#github"));
        // k8s: values become secretKeyRef (no literal value), namespace prefix dropped.
        let slack = entry("SLACK_TOKEN").unwrap();
        assert!(slack.get("value").is_none());
        assert_eq!(slack.pointer("/valueFrom/secretKeyRef/name"), Some(&json!("scli-secret")));
        assert_eq!(slack.pointer("/valueFrom/secretKeyRef/key"), Some(&json!("token")));
        assert_eq!(
            entry("YOUTRACK_TOKEN").unwrap().pointer("/valueFrom/secretKeyRef/name"),
            Some(&json!("yt-secret"))
        );
        // env is stripped from TASK_PAYLOAD_JSON so the daemon can't re-apply refs.
        let tp = entry("TASK_PAYLOAD_JSON").unwrap()["value"].as_str().unwrap().to_owned();
        assert!(!tp.contains("vault:"), "unresolved ref leaked into payload: {tp}");
        assert!(!tp.contains("GITHUB_TOKEN"), "env leaked into payload: {tp}");
        assert!(tp.contains("pr-review"));
    }
}
