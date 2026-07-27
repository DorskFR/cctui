//! Kubernetes Job spawn mechanics for the standalone kube dispatcher.
//!
//! This dispatcher is a neutral profile-instantiator: a dispatch may only
//! *select* an operator-authored [`WorkerProfile`] by name and carry runtime
//! data (session token, payload, ephemeral machine key). It never accepts raw
//! pod-spec fields — the agent inside the worker influences the request, so any
//! override surface would let it reshape its own sandbox.
//!
//! Instantiation is mechanical: the worker container is built from the profile
//! (`image`/`command`/`args`/`resources`/`env`/`envFrom`/`volumeMounts`, named
//! [`WorkerProfileSpec::worker_container_name`]); everything else on the profile
//! (extra containers, init containers, volumes, pull secrets, node selector,
//! runtime class, service account, pod annotations) is passed through untouched.
//! Profile `podAnnotations` land on the pod template metadata; the dispatcher's
//! own `cctui.dev/*` session annotations win on key conflict. The dispatcher
//! adds **no** sidecars, security contexts, or credential plumbing — a mutating
//! admission webhook injects the sandbox at pod admission, keyed off the
//! stamped `cctui.dev/worker-*` labels/annotations. Secret refs are resolved by
//! the guard-proxy sidecar, not here: a secret-ref-shaped env value in the
//! payload is rejected outright.
//!
//! Orthogonal Job mechanics are unchanged:
//! - Job name = `cctui-worker-<sha1(dedup_key||session_id)[:12]>` so a repeat
//!   dispatch of the same logical key maps to the same Job.
//! - 409 on create → read the existing Job: in-flight ⇒ `deduplicated`;
//!   terminal (Complete/Failed) ⇒ delete + recreate ⇒ `redispatched`.
//! - `cctui_machine_key` is lifted out of the payload into `CCTUI_MACHINE_KEY`
//!   (runtime identity, not a stored secret) and kept OUT of `TASK_PAYLOAD_JSON`.
//! - reply_url → `REPLY_URL` env so the terminal callback fires.
//!
//! ⚠️ Repo is PUBLIC — no homelab namespaces/images/registries here; the
//! namespace + profile come from the dispatcher's own config / the request.
#![allow(clippy::doc_markdown)]

use std::time::Duration;

use cctui_orchestrator::{WorkerProfile, WorkerProfileSpec};
use cctui_proto::ws::WireDispatchSpec;
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::Pod;
use kube::api::{Api, DeleteParams, ListParams, PostParams, PropagationPolicy};
use kube::{Client, Error as KubeError};

/// A pod wedged `Pending` longer than this (no schedulable node / image still
/// failing to pull) is reported `Failed` rather than `Running`.
const PENDING_FAILURE_SECS: i64 = 300;

/// Auto-reap a finished worker Job (`Complete`/`Failed`) this many seconds after
/// it stops, via `spec.ttlSecondsAfterFinished`. Long enough to inspect a
/// just-finished run, short enough to keep the namespace free of corpses
/// (; was 86400 = 24h).
const JOB_TTL_SECONDS: i64 = 3600;

/// Default worker lifetime when the dispatch carries no `timeout_minutes`: 1h.
/// A dispatched session that finishes but never signals done should cost at most
/// an hour. Overridable via `CCTUI_WORKER_DEFAULT_TIMEOUT_MINUTES`; a
/// per-dispatch `timeout_minutes` always wins.
const DEFAULT_TIMEOUT_MINUTES: u32 = 60;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

const LABEL_ORIGIN: &str = "cctui.dev/origin";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";
const ANNOTATION_SESSION_ID: &str = "cctui.dev/session-id";
/// Contract with the injection webhook — these exact strings are what it keys
/// off to find the profile, the worker container, and the sandbox toggles.
const LABEL_WORKER_PROFILE: &str = "cctui.dev/worker-profile";
const ANNOTATION_WORKER_CONTAINER: &str = "cctui.dev/worker-container";
const ANNOTATION_GUARD_IDENTITY: &str = "cctui.dev/guard-identity";
const ANNOTATION_GPG_SIGNING: &str = "cctui.dev/gpg-signing";

/// Env-value prefixes reserved for secret references. A dispatch carrying one in
/// `payload.env` is rejected: secrets flow through the guard-proxy sidecar, and
/// a ref reaching pod env could be resolved by cluster machinery into the worker.
const SECRET_REF_PREFIXES: [&str; 3] = ["vault:", "bao:", "k8s:"];

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
    default_profile: String,
    cctui_url: String,
    client: Client,
    /// Ceiling on concurrently in-flight worker Jobs. `0` ⇒ unlimited.
    max_inflight: usize,
    /// `activeDeadlineSeconds` applied when a dispatch has no `timeout_minutes`.
    default_deadline_secs: i64,
}

impl Spawner {
    /// Connect using in-cluster config (the pod's projected ServiceAccount
    /// token) — or the local kubeconfig when run off-cluster — so a missing
    /// kube context fails loudly at startup rather than on first dispatch.
    pub async fn connect(
        namespace: String,
        default_profile: String,
        cctui_url: String,
    ) -> anyhow::Result<Self> {
        let client = Client::try_default().await?;
        // `CCTUI_WORKER_MAX_INFLIGHT`, `0`/unset ⇒ unlimited. An explicit cap
        // rejects a dispatch once that many non-terminal dispatcher-owned Jobs
        // exist, so a flood of DISTINCT keys can't exhaust the cluster.
        let max_inflight = std::env::var("CCTUI_WORKER_MAX_INFLIGHT")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let default_deadline_secs = Self::default_deadline_secs(
            std::env::var("CCTUI_WORKER_DEFAULT_TIMEOUT_MINUTES").ok().as_deref(),
        );
        Ok(Self {
            namespace,
            default_profile,
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
    /// dispatch maps to the same Job (idempotency key, 207). The prefix
    /// is `cctui-worker-` (the legacy `claude-worker-` name was renamed under the
    /// cctui unification); derived solely here, so dedup/status/delete
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

    /// Resolve the profile name a dispatch selects: the wire `profile` field,
    /// else a `profile` string in the payload, else the configured default.
    /// A dispatch may only ever SELECT a profile — never supply its shape.
    fn resolve_profile_name(spec: &WireDispatchSpec, default_profile: &str) -> Option<String> {
        if let Some(p) = spec.profile.as_deref().map(str::trim).filter(|p| !p.is_empty()) {
            return Some(p.to_owned());
        }
        if let Some(p) = spec
            .payload
            .get("profile")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            return Some(p.to_owned());
        }
        let d = default_profile.trim();
        (!d.is_empty()).then(|| d.to_owned())
    }

    /// Instantiate a one-shot Job from a `WorkerProfile` + this dispatch's
    /// runtime data. Mechanical: the worker container comes from the profile's
    /// first-class fields; every other container/volume/scheduling field is
    /// passed through. No sidecars/security contexts are added — the admission
    /// webhook injects the sandbox, keyed off the stamped labels/annotations.
    fn build_job(
        cctui_url: &str,
        profile_name: &str,
        profile: &WorkerProfileSpec,
        spec: &WireDispatchSpec,
        name: &str,
        default_deadline_secs: i64,
    ) -> anyhow::Result<Job> {
        let mut payload = spec.payload.clone();
        let obj = payload.as_object_mut();
        let machine_key = obj
            .as_ref()
            .and_then(|o| o.get("cctui_machine_key"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        let identity = obj
            .as_ref()
            .and_then(|o| o.get("identity"))
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
            .map(ToOwned::to_owned);
        let task_name =
            obj.as_ref().and_then(|o| o.get("name")).and_then(Value::as_str).map(ToOwned::to_owned);
        let env_map = payload.as_object_mut().and_then(|o| {
            o.remove("cctui_machine_key");
            o.remove("profile");
            o.remove("env")
        });
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

        // Plain payload env → literal worker vars. A secret-ref-shaped value is
        // rejected — secrets flow through the guard-proxy sidecar, never here.
        if let Some(Value::Object(m)) = env_map {
            for (k, v) in m {
                let Some(val) = v.as_str() else { continue };
                if let Some(prefix) = SECRET_REF_PREFIXES.iter().find(|p| val.starts_with(**p)) {
                    anyhow::bail!(
                        "payload env `{k}` uses secret-ref prefix `{prefix}` — secret refs are \
                         resolved by the guard-proxy sidecar, not passed through worker env"
                    );
                }
                overrides.push((k, val.to_owned()));
            }
        }

        let worker_name = profile.worker_container_name().to_owned();
        let pod_spec = Self::pod_spec(profile, &worker_name, &overrides)?;

        let mut labels = serde_json::Map::new();
        labels.insert(LABEL_ORIGIN.into(), json!("cctui-kube-dispatcher"));
        labels.insert(LABEL_SESSION_ID.into(), json!(Self::label_safe(&spec.session_id)));
        labels.insert(LABEL_WORKER_PROFILE.into(), json!(Self::label_safe(profile_name)));

        let mut annotations = serde_json::Map::new();
        for (k, v) in profile.pod_annotations.iter().flatten() {
            annotations.insert(k.clone(), json!(v));
        }
        annotations.insert(ANNOTATION_SESSION_ID.into(), json!(spec.session_id));
        annotations.insert(ANNOTATION_WORKER_CONTAINER.into(), json!(worker_name));
        if let Some(identity) = identity {
            annotations.insert(ANNOTATION_GUARD_IDENTITY.into(), json!(identity));
        }
        if profile.gpg_signing {
            annotations.insert(ANNOTATION_GPG_SIGNING.into(), json!("true"));
        }

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
                "ttlSecondsAfterFinished": JOB_TTL_SECONDS,
                "activeDeadlineSeconds": deadline,
                "template": {
                    "metadata": { "labels": labels, "annotations": annotations },
                    "spec": pod_spec,
                },
            },
        });

        Ok(serde_json::from_value(job_json)?)
    }

    /// Assemble the pod `spec`: the worker container from the profile's
    /// first-class fields (with per-run env upserted) followed by every
    /// passthrough field verbatim. `restartPolicy` is always `Never`.
    fn pod_spec(
        profile: &WorkerProfileSpec,
        worker_name: &str,
        overrides: &[(String, String)],
    ) -> anyhow::Result<serde_json::Map<String, Value>> {
        let mut worker = serde_json::Map::new();
        worker.insert("name".into(), json!(worker_name));
        worker.insert("image".into(), json!(profile.image));
        if let Some(command) = &profile.command {
            worker.insert("command".into(), serde_json::to_value(command)?);
        }
        if let Some(args) = &profile.args {
            worker.insert("args".into(), serde_json::to_value(args)?);
        }
        if let Some(resources) = &profile.resources {
            worker.insert("resources".into(), serde_json::to_value(resources)?);
        }
        if let Some(env) = &profile.env {
            worker.insert("env".into(), serde_json::to_value(env)?);
        }
        if let Some(env_from) = &profile.env_from {
            worker.insert("envFrom".into(), serde_json::to_value(env_from)?);
        }
        if let Some(mounts) = &profile.volume_mounts {
            worker.insert("volumeMounts".into(), serde_json::to_value(mounts)?);
        }
        Self::merge_env(&mut worker, overrides);

        let mut containers = vec![Value::Object(worker)];
        for extra in profile.containers.iter().flatten() {
            containers.push(serde_json::to_value(extra)?);
        }

        let mut pod_spec = serde_json::Map::new();
        pod_spec.insert("restartPolicy".into(), json!("Never"));
        pod_spec.insert("containers".into(), Value::Array(containers));
        if let Some(init) = &profile.init_containers {
            pod_spec.insert("initContainers".into(), serde_json::to_value(init)?);
        }
        if let Some(volumes) = &profile.volumes {
            pod_spec.insert("volumes".into(), serde_json::to_value(volumes)?);
        }
        if let Some(ips) = &profile.image_pull_secrets {
            pod_spec.insert("imagePullSecrets".into(), serde_json::to_value(ips)?);
        }
        if let Some(ns) = &profile.node_selector {
            pod_spec.insert("nodeSelector".into(), serde_json::to_value(ns)?);
        }
        if let Some(rc) = &profile.runtime_class_name {
            pod_spec.insert("runtimeClassName".into(), json!(rc));
        }
        if let Some(sa) = &profile.service_account_name {
            pod_spec.insert("serviceAccountName".into(), json!(sa));
        }
        Ok(pod_spec)
    }

    /// Upsert env vars by name into the container's `env` array, preserving the
    /// profile's existing entries (including `valueFrom`).
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

    /// The string the Job name derives from: the caller's `dedup_key` (the
    /// logical request id) when present, else the `session_id`. Keeping
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

        let profile_name =
            Self::resolve_profile_name(spec, &self.default_profile).ok_or_else(|| {
                anyhow::anyhow!("no worker profile selected and no default_profile set")
            })?;
        let profiles: Api<WorkerProfile> = Api::namespaced(self.client.clone(), &self.namespace);
        let profile = profiles
            .get(&profile_name)
            .await
            .map_err(|e| anyhow::anyhow!("reading WorkerProfile `{profile_name}`: {e}"))?;

        let name = Self::job_name(Self::dedup_source(spec));
        self.enforce_inflight_cap(&name).await?;
        let job = Self::build_job(
            &self.cctui_url,
            &profile_name,
            &profile.spec,
            spec,
            &name,
            self.default_deadline_secs,
        )?;

        match self.create(&job, "dispatched", &name).await {
            Ok(h) => return Ok(h),
            Err(KubeError::Api(e)) if e.code == 409 => {}
            Err(e) => anyhow::bail!("creating Job: {e}"),
        }

        // 409: a prior dispatch of this session already made the Job. Dedup vs.
        // redispatch depends on whether that Job is terminal.
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
    /// already exist. `0` ⇒ unlimited (the default; no behavior
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

    /// Lifecycle of a Job handle, plus a human reason when it FAILED.
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

    /// Inspect the Job's pods for a terminal-but-not-yet-Job-Failed condition:
    /// a crash-looping / OOMKilled / un-pullable container, or a pod
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
            profile: None,
            payload,
        }
    }

    fn lean_profile() -> WorkerProfileSpec {
        serde_json::from_value(json!({
            "image": "example.com/worker:latest",
            "serviceAccountName": "worker-lean",
            "env": [
                { "name": "TASK_ID", "value": "" },
                { "name": "LOG_LEVEL", "value": "info" }
            ]
        }))
        .unwrap()
    }

    fn full_profile() -> WorkerProfileSpec {
        serde_json::from_value(json!({
            "image": "example.com/worker:latest",
            "command": ["/entrypoint"],
            "args": ["--serve"],
            "workerContainer": "agent",
            "serviceAccountName": "worker-full",
            "gpgSigning": true,
            "runtimeClassName": "gvisor",
            "resources": { "requests": { "cpu": "2", "memory": "4Gi" } },
            "env": [{ "name": "LOG_LEVEL", "value": "info" }],
            "envFrom": [{ "configMapRef": { "name": "worker-config" } }],
            "volumeMounts": [
                { "name": "logs", "mountPath": "/var/log/worker" },
                { "name": "shim", "mountPath": "/usr/local/bin/shim.sh", "subPath": "shim.sh" }
            ],
            "podAnnotations": {
                "example.dev/role": "worker",
                "cctui.dev/session-id": "profile-should-lose"
            },
            "nodeSelector": { "kubernetes.io/arch": "amd64" },
            "imagePullSecrets": [{ "name": "registry-pull" }],
            "volumes": [
                { "name": "db-data", "emptyDir": {} },
                { "name": "cache", "emptyDir": {} }
            ],
            "initContainers": [
                { "name": "migrate", "image": "example.com/db-migrate:latest" },
                { "name": "seed", "image": "example.com/db-seed:latest" }
            ],
            "containers": [
                { "name": "db", "image": "example.com/postgres:16" },
                { "name": "auth-idp", "image": "example.com/auth-idp:latest" }
            ]
        }))
        .unwrap()
    }

    fn build(profile_name: &str, profile: &WorkerProfileSpec, spec: &WireDispatchSpec) -> Value {
        let name = Spawner::job_name(&spec.session_id);
        let job = Spawner::build_job(
            "http://cctui.example.svc.cluster.local:8700",
            profile_name,
            profile,
            spec,
            &name,
            3600,
        )
        .unwrap();
        serde_json::to_value(&job).unwrap()
    }

    fn worker_env(v: &Value) -> Vec<Value> {
        v.pointer("/spec/template/spec/containers/0/env").unwrap().as_array().unwrap().clone()
    }

    fn env_value(v: &Value, key: &str) -> Option<String> {
        worker_env(v)
            .iter()
            .find(|e| e["name"] == key)
            .and_then(|e| e.get("value"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
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
        let mut s1 = spec("11111111-1111-4111-8111-111111111111", json!({}));
        s1.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        let mut s2 = spec("22222222-2222-4222-8222-222222222222", json!({}));
        s2.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        assert_eq!(
            Spawner::job_name(Spawner::dedup_source(&s1)),
            Spawner::job_name(Spawner::dedup_source(&s2)),
            "same dedup key => same Job despite distinct session ids",
        );
        let s3 = spec("33333333-3333-4333-8333-333333333333", json!({}));
        assert_eq!(Spawner::dedup_source(&s3), "33333333-3333-4333-8333-333333333333");
        assert_ne!(
            Spawner::job_name(Spawner::dedup_source(&s1)),
            Spawner::job_name(Spawner::dedup_source(&s3)),
        );
    }

    #[test]
    fn label_safe_sanitizes_separators() {
        assert_eq!(Spawner::label_safe("triage:PROJ:2026"), "triage-PROJ-2026");
        assert_eq!(Spawner::label_safe(":::"), "session");
        assert_eq!(Spawner::label_safe(&"x".repeat(100)).len(), 63);
    }

    #[test]
    fn resolve_profile_name_prefers_wire_then_payload_then_default() {
        let mut s = spec("sess", json!({ "profile": "from-payload" }));
        s.profile = Some("from-wire".to_owned());
        assert_eq!(Spawner::resolve_profile_name(&s, "def").as_deref(), Some("from-wire"));

        let s = spec("sess", json!({ "profile": "from-payload" }));
        assert_eq!(Spawner::resolve_profile_name(&s, "def").as_deref(), Some("from-payload"));

        let s = spec("sess", json!({}));
        assert_eq!(Spawner::resolve_profile_name(&s, "def").as_deref(), Some("def"));

        let s = spec("sess", json!({}));
        assert_eq!(Spawner::resolve_profile_name(&s, "   "), None);
    }

    #[test]
    fn lean_profile_instantiates_worker_env_and_stamps_contract() {
        let payload = json!({
            "flow": "review",
            "name": "Review #5697",
            "profile": "lean",
            "cctui_machine_key": "SECRET"
        });
        let s = spec("sess-123", payload);
        let v = build("lean", &lean_profile(), &s);

        assert_eq!(env_value(&v, "SESSION_ID").as_deref(), Some("sess-123"));
        assert_eq!(env_value(&v, "TASK_ID").as_deref(), Some("sess-123"));
        assert_eq!(env_value(&v, "TASK_NAME").as_deref(), Some("Review #5697"));
        assert_eq!(env_value(&v, "CCTUI_MACHINE_KEY").as_deref(), Some("SECRET"));
        assert_eq!(env_value(&v, "REPLY_URL").as_deref(), Some("https://cb"));
        assert_eq!(env_value(&v, "LOG_LEVEL").as_deref(), Some("info"));
        assert_eq!(
            env_value(&v, "CCTUI_URL").as_deref(),
            Some("http://cctui.example.svc.cluster.local:8700")
        );

        let tp = env_value(&v, "TASK_PAYLOAD_JSON").unwrap();
        assert!(!tp.contains("SECRET"), "machine key leaked into payload: {tp}");
        assert!(!tp.contains("\"profile\""), "profile key leaked into payload: {tp}");
        assert!(tp.contains("review"));

        assert_eq!(v.pointer("/spec/template/spec/restartPolicy"), Some(&json!("Never")));
        assert_eq!(
            v.pointer("/spec/template/spec/serviceAccountName"),
            Some(&json!("worker-lean"))
        );
        let containers = v.pointer("/spec/template/spec/containers").unwrap().as_array().unwrap();
        assert_eq!(containers.len(), 1, "lean profile => only the worker container");
        assert_eq!(containers[0]["name"], json!("worker"));
        assert_eq!(containers[0]["image"], json!("example.com/worker:latest"));

        assert_eq!(
            v.pointer("/spec/template/metadata/labels/cctui.dev~1worker-profile"),
            Some(&json!("lean"))
        );
        assert_eq!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1worker-container"),
            Some(&json!("worker"))
        );
        assert_eq!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1session-id"),
            Some(&json!("sess-123"))
        );
        assert!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1gpg-signing").is_none(),
            "gpg annotation only when profile opts in"
        );
        assert!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1guard-identity").is_none(),
            "no identity => no guard-identity annotation"
        );

        assert_eq!(
            v.pointer("/metadata/labels/cctui.dev~1origin"),
            Some(&json!("cctui-kube-dispatcher"))
        );
        assert_eq!(v.pointer("/spec/backoffLimit"), Some(&json!(0)));
        assert_eq!(v.pointer("/spec/ttlSecondsAfterFinished"), Some(&json!(3600)));
        assert_eq!(v.pointer("/spec/activeDeadlineSeconds"), Some(&json!(1800)));
    }

    #[test]
    fn full_profile_passes_through_stack_and_stamps_gpg_and_identity() {
        let payload = json!({ "flow": "review", "identity": "zephyr" });
        let s = spec("sess-full", payload);
        let v = build("full-stack", &full_profile(), &s);

        let containers = v.pointer("/spec/template/spec/containers").unwrap().as_array().unwrap();
        assert_eq!(containers.len(), 3, "worker + two passthrough containers");
        assert_eq!(containers[0]["name"], json!("agent"), "worker container name from profile");
        assert_eq!(containers[0]["command"], json!(["/entrypoint"]));
        assert_eq!(containers[0]["args"], json!(["--serve"]));
        assert_eq!(
            containers[0]["envFrom"],
            json!([{ "configMapRef": { "name": "worker-config" } }])
        );
        assert_eq!(
            containers[0]["volumeMounts"],
            json!([
                { "name": "logs", "mountPath": "/var/log/worker" },
                { "name": "shim", "mountPath": "/usr/local/bin/shim.sh", "subPath": "shim.sh" }
            ])
        );
        assert_eq!(containers[1]["name"], json!("db"));
        assert_eq!(containers[2]["name"], json!("auth-idp"));

        let inits = v.pointer("/spec/template/spec/initContainers").unwrap().as_array().unwrap();
        assert_eq!(inits.len(), 2, "both init containers passed through");
        assert_eq!(inits[0]["name"], json!("migrate"));
        assert_eq!(inits[1]["name"], json!("seed"));

        let volumes = v.pointer("/spec/template/spec/volumes").unwrap().as_array().unwrap();
        assert_eq!(volumes.len(), 2);
        assert_eq!(v.pointer("/spec/template/spec/runtimeClassName"), Some(&json!("gvisor")));
        assert_eq!(
            v.pointer("/spec/template/spec/serviceAccountName"),
            Some(&json!("worker-full"))
        );
        assert_eq!(
            v.pointer("/spec/template/spec/imagePullSecrets/0/name"),
            Some(&json!("registry-pull"))
        );
        assert_eq!(
            v.pointer("/spec/template/spec/nodeSelector/kubernetes.io~1arch"),
            Some(&json!("amd64"))
        );

        assert_eq!(
            v.pointer("/spec/template/metadata/labels/cctui.dev~1worker-profile"),
            Some(&json!("full-stack"))
        );
        assert_eq!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1worker-container"),
            Some(&json!("agent"))
        );
        assert_eq!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1gpg-signing"),
            Some(&json!("true"))
        );
        assert_eq!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1guard-identity"),
            Some(&json!("zephyr")),
            "identity goes on the pod annotation, not container env"
        );
        assert_eq!(
            v.pointer("/spec/template/metadata/annotations/example.dev~1role"),
            Some(&json!("worker")),
            "profile podAnnotations land on the pod template"
        );
        assert_eq!(
            v.pointer("/spec/template/metadata/annotations/cctui.dev~1session-id"),
            Some(&json!("sess-full")),
            "dispatcher session annotation wins over a colliding profile podAnnotation"
        );
        assert!(
            !worker_env(&v).iter().any(|e| e["name"] == "GUARD_PROXY_IDENTITY"),
            "identity must never land on worker env"
        );
    }

    #[test]
    fn plain_payload_env_becomes_worker_env() {
        let payload = json!({
            "flow": "pr-review",
            "env": {
                "CONTEXT_PACK_URL": "https://github.com/acme/pack",
                "FEATURE_FLAG": "on"
            }
        });
        let s = spec("sess-9", payload);
        let v = build("lean", &lean_profile(), &s);
        assert_eq!(
            env_value(&v, "CONTEXT_PACK_URL").as_deref(),
            Some("https://github.com/acme/pack")
        );
        assert_eq!(env_value(&v, "FEATURE_FLAG").as_deref(), Some("on"));
        let tp = env_value(&v, "TASK_PAYLOAD_JSON").unwrap();
        assert!(!tp.contains("CONTEXT_PACK_URL"), "env lifted out of payload: {tp}");
        assert!(tp.contains("pr-review"));
    }

    #[test]
    fn secret_ref_env_values_are_rejected() {
        for reference in ["vault:secret/data/ci#gh", "bao:secret/data/ci#gh", "k8s:s#k"] {
            let payload = json!({ "env": { "TOKEN": reference } });
            let s = spec("sess-x", payload);
            let name = Spawner::job_name("sess-x");
            let err =
                Spawner::build_job("http://cctui:8700", "lean", &lean_profile(), &s, &name, 3600)
                    .expect_err("secret-ref env must be rejected");
            let msg = err.to_string();
            assert!(msg.contains("secret-ref"), "unexpected error: {msg}");
        }
    }

    #[test]
    fn no_timeout_uses_default_deadline() {
        let mut s = spec("sess-none", json!({}));
        s.timeout_minutes = None;
        let v = build("lean", &lean_profile(), &s);
        assert_eq!(v.pointer("/spec/activeDeadlineSeconds"), Some(&json!(3600)));
    }

    #[test]
    fn per_dispatch_timeout_overrides_default_deadline() {
        let s = spec("sess-30", json!({}));
        let name = Spawner::job_name("sess-30");
        let job = Spawner::build_job("http://cctui:8700", "lean", &lean_profile(), &s, &name, 7200)
            .unwrap();
        let v = serde_json::to_value(&job).unwrap();
        assert_eq!(v.pointer("/spec/activeDeadlineSeconds"), Some(&json!(1800)));
    }

    #[test]
    fn default_deadline_env_parsing() {
        assert_eq!(Spawner::default_deadline_secs(None), 3600);
        assert_eq!(Spawner::default_deadline_secs(Some("30")), 1800);
        assert_eq!(Spawner::default_deadline_secs(Some(" 120 ")), 7200);
        assert_eq!(Spawner::default_deadline_secs(Some("nope")), 3600);
        assert_eq!(Spawner::default_deadline_secs(Some("0")), 3600);
    }
}
