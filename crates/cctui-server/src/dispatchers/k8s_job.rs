//! K8s Job dispatcher.
//!
//! Reads an existing worker `CronJob` (the single source of truth for the
//! worker pod spec, maintained out-of-band in your own GitOps/kustomize
//! repo) and clones its
//! `jobTemplate` into a one-shot Job, overriding env vars + name +
//! labels per intent. The worker container is the first in
//! `spec.template.spec.containers`; we leave init containers, sidecars,
//! volumes, mounts, security context, secret-injection annotations etc.
//! untouched.
//!
//! Why "clone the `CronJob`" instead of "render a template"? The `CronJob`
//! is already templated by your deploy tooling (limits, secret-injection
//! annotations, image pull secrets, etc.). Reimplementing that in a Rust
//! string template would dual-source the spec and rot quickly.
//!
//! In-cluster auth uses the projected SA token (`kube::Config::infer`
//! handles `KUBECONFIG` → in-cluster fallback). The SA needs `get
//! cronjobs` and `create/get/list jobs` in the worker namespace.

use async_trait::async_trait;
use k8s_openapi::api::batch::v1::{CronJob, Job, JobSpec};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use kube::api::PostParams;
use kube::{Api, Client};
use std::collections::BTreeMap;

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher};

/// Label-key prefix for the metadata cctui stamps on dispatched Jobs/pods so
/// downstream selectors (logs, archive) can find them.
const LABEL_ORIGIN: &str = "cctui.dev/origin";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";

#[derive(Clone)]
pub struct K8sJobDispatcher {
    client: Client,
    /// Namespace containing the source `CronJob` *and* receiving the Job.
    /// Both live in the same namespace; configured via `CCTUI_K8S_NAMESPACE`.
    namespace: String,
    /// Source `CronJob` name to clone the pod template from.
    source_cronjob: String,
    /// `cctui-server` external URL forwarded to the worker so its
    /// in-pod daemon (once CCT-107 follow-up lands) can phone home.
    cctui_url: String,
}

impl K8sJobDispatcher {
    pub async fn try_new(
        namespace: impl Into<String>,
        source_cronjob: impl Into<String>,
        cctui_url: impl Into<String>,
    ) -> anyhow::Result<Self> {
        let client = Client::try_default().await?;
        Ok(Self {
            client,
            namespace: namespace.into(),
            source_cronjob: source_cronjob.into(),
            cctui_url: cctui_url.into(),
        })
    }

    /// Build the one-shot Job spec by cloning the source `CronJob`'s
    /// `jobTemplate.spec.template.spec` and overriding env on the first
    /// container.
    fn render_job(
        cronjob: &CronJob,
        job_name: &str,
        spec: &DispatchSpec<'_>,
        cctui_url: &str,
    ) -> Result<Job, DispatchError> {
        let jt =
            cronjob.spec.as_ref().and_then(|s| s.job_template.spec.clone()).ok_or_else(|| {
                DispatchError::Backend("source CronJob has no jobTemplate.spec".into())
            })?;

        let mut pod_template = jt.template.clone();
        let pod_spec = pod_template.spec.as_mut().ok_or_else(|| {
            DispatchError::Backend("source CronJob jobTemplate.spec.template has no spec".into())
        })?;

        let worker = pod_spec.containers.get_mut(0).ok_or_else(|| {
            DispatchError::Backend("source CronJob has no worker container".into())
        })?;

        // Strip the CronJob's `command: ["sleep", "infinity"]` debug
        // override so the image's real ENTRYPOINT runs.
        worker.command = None;
        worker.args = None;

        let overrides = worker_env(spec, cctui_url);
        merge_env(worker.env.get_or_insert_with(Vec::new), overrides);

        // Label the pod template so downstream selectors (logs, archive)
        // can find sessions launched by this dispatcher. cctui stays opaque
        // to `payload`, so only session-level identifiers are labelled.
        let labels = pod_template
            .metadata
            .get_or_insert_with(ObjectMeta::default)
            .labels
            .get_or_insert_with(BTreeMap::new);
        labels.insert(LABEL_ORIGIN.into(), "k8s_job".into());
        labels.insert(LABEL_SESSION_ID.into(), spec.session_id.into());

        // Per-flow timeout wins over the source CronJob's default deadline.
        let active_deadline_seconds =
            spec.timeout_minutes.map(|m| i64::from(m) * 60).or(jt.active_deadline_seconds);

        let job_spec = JobSpec {
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(86_400),
            active_deadline_seconds,
            template: pod_template,
            ..Default::default()
        };

        Ok(Job {
            metadata: ObjectMeta {
                name: Some(job_name.to_string()),
                labels: Some(BTreeMap::from([
                    (LABEL_ORIGIN.into(), "k8s_job".into()),
                    (LABEL_SESSION_ID.into(), spec.session_id.into()),
                ])),
                ..Default::default()
            },
            spec: Some(job_spec),
            ..Default::default()
        })
    }
}

#[async_trait]
impl Dispatcher for K8sJobDispatcher {
    fn id(&self) -> &'static str {
        "k8s_job"
    }

    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError> {
        validate(spec)?;

        let cronjobs: Api<CronJob> = Api::namespaced(self.client.clone(), &self.namespace);
        let source = cronjobs.get(&self.source_cronjob).await.map_err(|e| {
            DispatchError::Backend(format!(
                "fetching source CronJob {ns}/{name}: {e}",
                ns = self.namespace,
                name = self.source_cronjob,
            ))
        })?;

        let job_name = build_job_name(spec.session_id);
        let job = Self::render_job(&source, &job_name, spec, &self.cctui_url)?;

        let jobs: Api<Job> = Api::namespaced(self.client.clone(), &self.namespace);
        let created = jobs
            .create(&PostParams::default(), &job)
            .await
            .map_err(|e| DispatchError::Backend(format!("creating Job {job_name}: {e}")))?;

        let handle = format!("jobs/{}", created.metadata.name.unwrap_or_else(|| job_name.clone()));
        tracing::info!(session_id = %spec.session_id, %handle, ns = %self.namespace, "k8s_job dispatched");

        Ok(DispatchHandle { handle, namespace: Some(self.namespace.clone()) })
    }
}

fn validate(spec: &DispatchSpec<'_>) -> Result<(), DispatchError> {
    if spec.session_id.is_empty() {
        return Err(DispatchError::InvalidIntent("session_id is required".into()));
    }
    // `payload` is opaque to cctui, but the worker unpacks it as a JSON
    // object (jq). Reject shapes the worker can't unpack so the caller
    // gets a 400 here instead of a silent job that does nothing.
    if !spec.payload.is_null() && !spec.payload.is_object() {
        return Err(DispatchError::InvalidIntent("payload must be a JSON object".into()));
    }
    Ok(())
}

/// `claude-worker-<session8>` — DNS-1123 compatible, deterministic from
/// `session_id` so re-dispatching with the same id collides at the API
/// server (defense-in-depth behind the route's idempotency short-circuit).
fn build_job_name(session_id: &str) -> String {
    let session_short = session_id.replace('-', "").chars().take(8).collect::<String>();
    let mut name = format!("claude-worker-{session_short}");
    name.truncate(63); // DNS-1123 label limit
    name.trim_end_matches('-').to_string()
}

/// Build the env-var overrides that get merged into the worker container.
/// Keys here win over any value already set on the `CronJob` template.
///
/// `payload` is forwarded verbatim as a single JSON blob (`TASK_PAYLOAD_JSON`);
/// the worker's entrypoint unpacks it into the `TASK_*` vars it needs. cctui
/// never inspects it.
fn worker_env(spec: &DispatchSpec<'_>, cctui_url: &str) -> Vec<(String, String)> {
    let mut envs = vec![
        ("CCTUI_SESSION_ID".into(), spec.session_id.to_string()),
        ("CCTUI_URL".into(), cctui_url.to_string()),
        ("TASK_ID".into(), spec.session_id.to_string()),
    ];
    if !spec.payload.is_null() {
        // Compact form so the env value is a single line.
        envs.push(("TASK_PAYLOAD_JSON".into(), spec.payload.to_string()));
    }
    if let Some(url) = spec.reply_url {
        // Bearer capability: forwarded to the worker for the CCT-119
        // callback, never logged here.
        envs.push(("REPLY_URL".into(), url.to_string()));
    }
    envs
}

/// Merge `overrides` into `existing` in-place, replacing values that
/// already exist by name. Preserves all envs in the source `CronJob` that
/// the dispatcher doesn't touch (injected secrets etc.).
fn merge_env(
    existing: &mut Vec<k8s_openapi::api::core::v1::EnvVar>,
    overrides: Vec<(String, String)>,
) {
    use k8s_openapi::api::core::v1::EnvVar;
    for (k, v) in overrides {
        if let Some(slot) = existing.iter_mut().find(|e| e.name == k) {
            slot.value = Some(v);
            slot.value_from = None;
        } else {
            existing.push(EnvVar { name: k, value: Some(v), value_from: None });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn payload() -> serde_json::Value {
        json!({
            "trigger": "github-review-requested",
            "repo": "example-repo",
            "prompt_file": "review-example-repo.md",
            "model": "claude-opus-4-7",
            "context": {"pr_number": 5528},
        })
    }

    fn spec<'a>(session_id: &'a str, payload: &'a serde_json::Value) -> DispatchSpec<'a> {
        DispatchSpec { session_id, timeout_minutes: Some(45), reply_url: None, payload }
    }

    #[test]
    fn validate_accepts_object_payload() {
        let p = payload();
        validate(&spec("sess-1", &p)).unwrap();
    }

    #[test]
    fn validate_accepts_null_payload() {
        let p = serde_json::Value::Null;
        validate(&spec("sess-1", &p)).unwrap();
    }

    #[test]
    fn validate_rejects_empty_session_id() {
        let p = payload();
        assert!(matches!(validate(&spec("", &p)), Err(DispatchError::InvalidIntent(_))));
    }

    #[test]
    fn validate_rejects_non_object_payload() {
        let p = json!([1, 2, 3]);
        assert!(matches!(validate(&spec("sess-1", &p)), Err(DispatchError::InvalidIntent(_))));
        let p = json!("a string");
        assert!(matches!(validate(&spec("sess-1", &p)), Err(DispatchError::InvalidIntent(_))));
    }

    #[test]
    fn job_name_is_dns_safe_and_deterministic() {
        let name = build_job_name("550e8400-e29b-41d4-a716-446655440000");
        assert_eq!(name, "claude-worker-550e8400");
        assert!(name.len() <= 63);
        assert!(name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        // Same input → same output (idempotency story).
        assert_eq!(name, build_job_name("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn worker_env_forwards_payload_and_reply_url() {
        let p = payload();
        let s = DispatchSpec {
            session_id: "sess-1",
            timeout_minutes: Some(45),
            reply_url: Some("https://automation.example/resume/abc"),
            payload: &p,
        };
        let envs = worker_env(&s, "https://cctui.example.com");
        let map: std::collections::HashMap<_, _> = envs.into_iter().collect();
        assert_eq!(map["CCTUI_SESSION_ID"], "sess-1");
        assert_eq!(map["TASK_ID"], "sess-1");
        assert_eq!(map["REPLY_URL"], "https://automation.example/resume/abc");
        // payload forwarded verbatim, not unpacked into TASK_REPO etc.
        assert!(map["TASK_PAYLOAD_JSON"].contains("example-repo"));
        assert!(!map.contains_key("TASK_REPO"));
    }

    #[test]
    fn worker_env_omits_payload_and_reply_url_when_absent() {
        let p = serde_json::Value::Null;
        let s = DispatchSpec {
            session_id: "sess-1",
            timeout_minutes: None,
            reply_url: None,
            payload: &p,
        };
        let map: std::collections::HashMap<_, _> =
            worker_env(&s, "https://cctui.example.com").into_iter().collect();
        assert!(!map.contains_key("TASK_PAYLOAD_JSON"));
        assert!(!map.contains_key("REPLY_URL"));
    }

    #[test]
    fn merge_env_replaces_existing_and_appends_new() {
        use k8s_openapi::api::core::v1::EnvVar;
        let mut existing = vec![
            EnvVar { name: "TASK_REPO".into(), value: Some("old".into()), value_from: None },
            EnvVar { name: "SECRET_X".into(), value: Some("keep".into()), value_from: None },
        ];
        merge_env(
            &mut existing,
            vec![("TASK_REPO".into(), "new".into()), ("TASK_NEW".into(), "added".into())],
        );
        let map: std::collections::HashMap<_, _> =
            existing.iter().map(|e| (e.name.clone(), e.value.clone().unwrap())).collect();
        assert_eq!(map["TASK_REPO"], "new");
        assert_eq!(map["SECRET_X"], "keep");
        assert_eq!(map["TASK_NEW"], "added");
    }

    #[test]
    fn render_job_clones_template_and_overrides_env() {
        use k8s_openapi::api::batch::v1::{CronJobSpec, JobTemplateSpec};
        use k8s_openapi::api::core::v1::{Container, EnvVar, PodSpec, PodTemplateSpec};

        let cronjob = CronJob {
            metadata: ObjectMeta { name: Some("claude-worker".into()), ..Default::default() },
            spec: Some(CronJobSpec {
                schedule: "0 0 31 2 *".into(),
                job_template: JobTemplateSpec {
                    spec: Some(JobSpec {
                        active_deadline_seconds: Some(86_400),
                        template: PodTemplateSpec {
                            spec: Some(PodSpec {
                                containers: vec![Container {
                                    name: "worker".into(),
                                    command: Some(vec!["sleep".into(), "infinity".into()]),
                                    env: Some(vec![EnvVar {
                                        name: "PRE_EXISTING".into(),
                                        value: Some("yes".into()),
                                        value_from: None,
                                    }]),
                                    ..Default::default()
                                }],
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                ..Default::default()
            }),
            ..Default::default()
        };

        let p = payload();
        let job = K8sJobDispatcher::render_job(
            &cronjob,
            "claude-worker-abc12345",
            &spec("abc12345-...", &p),
            "https://cctui.example.com",
        )
        .unwrap();

        // The debug `sleep infinity` override is gone.
        let worker = &job.spec.as_ref().unwrap().template.spec.as_ref().unwrap().containers[0];
        assert!(worker.command.is_none(), "expected ENTRYPOINT to win, got {:?}", worker.command);

        let envs: std::collections::HashMap<_, _> = worker
            .env
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| (e.name.clone(), e.value.clone().unwrap_or_default()))
            .collect();
        assert_eq!(envs["PRE_EXISTING"], "yes", "non-overridden env preserved");
        assert!(envs["TASK_PAYLOAD_JSON"].contains("example-repo"));
        assert_eq!(envs["CCTUI_SESSION_ID"], "abc12345-...");

        // Labels are set on both Job and PodTemplate so selectors work
        // at either level.
        let labels = job.metadata.labels.as_ref().unwrap();
        assert_eq!(labels[LABEL_ORIGIN], "k8s_job");
        assert_eq!(labels[LABEL_SESSION_ID], "abc12345-...");

        let pod_labels =
            job.spec.as_ref().unwrap().template.metadata.as_ref().unwrap().labels.as_ref().unwrap();
        assert_eq!(pod_labels[LABEL_ORIGIN], "k8s_job");

        // backoffLimit 0 + ttl preserved; per-flow timeout (45 min) overrides
        // the source CronJob's 86400s deadline.
        let js = job.spec.as_ref().unwrap();
        assert_eq!(js.backoff_limit, Some(0));
        assert_eq!(js.ttl_seconds_after_finished, Some(86_400));
        assert_eq!(js.active_deadline_seconds, Some(45 * 60));
    }
}
