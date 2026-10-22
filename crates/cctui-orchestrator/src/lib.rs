//! `WorkerProfile` custom resource: the operator-authored description of a
//! workload *shape* that the dispatcher instantiates and the injection webhook
//! augments.
//!
//! The profile author is trusted; the only adversary is the agent inside the
//! worker container, so the schema is a thin passthrough close to a
//! `PodTemplateSpec`, not an allowlist. A dispatch selects a profile *by name*
//! and only layers run-specific env onto the worker container; the webhook
//! sandboxes only that container. The worker container is the one named by
//! [`WorkerProfileSpec::worker_container_name`]; passthrough `containers` /
//! `init_containers` are left untouched. See `docs/worker-profile-crd.md`.

use k8s_openapi::api::core::v1::{
    Container, EnvFromSource, EnvVar, LocalObjectReference, ResourceRequirements, Volume,
    VolumeMount,
};
use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod envelope;
pub mod validate;

/// Container name the injection webhook sandboxes when a profile does not set
/// [`WorkerProfileSpec::worker_container`].
pub const DEFAULT_WORKER_CONTAINER: &str = "worker";

/// Pod label carrying the source profile name. Presence is the mutating
/// webhook's trigger; a pod without it is admitted unchanged.
pub const LABEL_WORKER_PROFILE: &str = "cctui.dev/worker-profile";

/// Pod annotation naming which container to sandbox. Absent means
/// [`DEFAULT_WORKER_CONTAINER`].
pub const ANNOTATION_WORKER_CONTAINER: &str = "cctui.dev/worker-container";

/// Pod annotation carrying the guard-proxy identity. When present the webhook
/// upserts `GUARD_PROXY_IDENTITY` on the injected sidecar, overriding the
/// ConfigMap-provided default.
pub const ANNOTATION_GUARD_IDENTITY: &str = "cctui.dev/guard-identity";

/// Pod annotation (`"true"`) requesting gpg-agent wiring, mirroring
/// [`WorkerProfileSpec::gpg_signing`].
pub const ANNOTATION_GPG_SIGNING: &str = "cctui.dev/gpg-signing";

/// Pod annotation naming the Job-owned Secret that carries the worker's
/// per-run secret env. Always [`env_secret_name`] of the pod's Job.
pub const ANNOTATION_ENV_SECRET: &str = "cctui.dev/env-secret";

/// Name of the per-dispatch env Secret for the Job `job_name`.
#[must_use]
pub fn env_secret_name(job_name: &str, nonce: &str) -> String {
    format!("{job_name}-env-{nonce}")
}

/// Pod annotation the webhook stamps (`"true"`) after injecting the envelope.
/// Its presence makes re-invocation a no-op.
pub const ANNOTATION_ENVELOPE_INJECTED: &str = "cctui.dev/envelope-injected";

/// Operator-authored shape of a worker workload.
///
/// Every field here is operator-owned; see the crate-level docs for the full
/// ownership contract (operator vs dispatcher vs webhook).
#[derive(CustomResource, Serialize, Deserialize, Debug, Clone, JsonSchema)]
#[kube(
    group = "cctui.dev",
    version = "v1alpha1",
    kind = "WorkerProfile",
    plural = "workerprofiles",
    shortname = "wprof",
    namespaced
)]
#[serde(rename_all = "camelCase")]
pub struct WorkerProfileSpec {
    /// Image for the worker container.
    pub image: String,

    /// Entrypoint override for the worker container (maps to a container's
    /// `command`). `None` keeps the image's `ENTRYPOINT`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,

    /// Arguments for the worker container (maps to a container's `args`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,

    /// Resource requests/limits for the worker container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourceRequirements>,

    /// Non-secret environment for the worker container. The dispatcher layers
    /// per-run env on top of this at Job creation; the webhook adds the
    /// secretless credential envelope at admission. Do not put secrets here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<EnvVar>>,

    /// `ConfigMap`/`Secret` env sources for the worker container (maps to a
    /// container's `envFrom`). Non-secret config only — the same rule as
    /// [`env`](Self::env).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_from: Option<Vec<EnvFromSource>>,

    /// Volume mounts for the worker container. These name operator-owned
    /// [`volumes`](Self::volumes); the webhook adds the envelope's own worker
    /// mounts on top at admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volume_mounts: Option<Vec<VolumeMount>>,

    /// Overrides which container the webhook treats as the worker. Defaults to
    /// [`DEFAULT_WORKER_CONTAINER`]. This is the only container the webhook
    /// sandboxes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_container: Option<String>,

    /// Extra sidecar containers making up the surrounding app stack. Passthrough
    /// — the webhook does not sandbox these.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub containers: Option<Vec<Container>>,

    /// Extra init containers. Passthrough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub init_containers: Option<Vec<Container>>,

    /// Extra volumes exposed to the pod. Passthrough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volumes: Option<Vec<Volume>>,

    /// Registry pull secrets for the pod. Passthrough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_pull_secrets: Option<Vec<LocalObjectReference>>,

    /// Node scheduling constraints. Passthrough.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node_selector: Option<BTreeMap<String, String>>,

    /// Annotations stamped onto every instantiated pod's template metadata (e.g.
    /// a role annotation an in-cluster mutating webhook consumes). The
    /// dispatcher's own `cctui.dev/*` session annotations win on key conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_annotations: Option<BTreeMap<String, String>>,

    /// Sandbox runtime class for the pod (e.g. gVisor/Kata). Passthrough; left
    /// here so a profile can opt into a hardened runtime later.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_class_name: Option<String>,

    /// `ServiceAccount` the pod runs as — the identity / secret scope mapping.
    /// Operator-owned: the dispatch request never sets this. `None` runs as the
    /// namespace `default` `ServiceAccount`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_account_name: Option<String>,

    /// Requests that the webhook wire a gpg-agent socket into the worker
    /// container for remote commit signing. Off by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gpg_signing: bool,

    /// Ceiling on concurrently *active* (unsuspended, non-terminal) Jobs
    /// instantiated from this profile. When reached, the dispatcher creates
    /// further Jobs suspended and resumes them oldest-first as slots free up.
    /// `None`/`0` ⇒ unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_inflight: Option<u32>,

    /// Per-profile guard-proxy settings, layered over the global
    /// `guard-proxy-env` `ConfigMap`. `None` renders the sidecar unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard: Option<GuardSpec>,

    /// Pre-flight isolation check the worker entrypoint runs before the agent
    /// starts. `None` runs the built-in probes in the default mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preflight: Option<PreflightSpec>,
}

/// Pre-flight isolation check settings for one profile.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct PreflightSpec {
    /// `enforce` fails the dispatch on any reachable probe, `warn` logs and
    /// continues, `off` skips the check. Unset enforces in the sandboxed pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,

    /// Extra targets the agent must not reach, on top of the built-in set:
    /// `host:port` (`[addr]:port` for IPv6), or a CIDR whose first host is
    /// probed on port 80.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deny: Option<Vec<String>>,
}

impl PreflightSpec {
    /// Worker env the entrypoint reads the settings from.
    #[must_use]
    pub fn worker_env(&self) -> Vec<(&'static str, String)> {
        let mut env = Vec::new();
        if let Some(mode) = self.mode.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
            env.push((PREFLIGHT_MODE_ENV, mode.to_owned()));
        }
        let deny: Vec<&str> = self
            .deny
            .iter()
            .flatten()
            .map(String::as_str)
            .map(str::trim)
            .filter(|d| !d.is_empty())
            .collect();
        if !deny.is_empty() {
            env.push((PREFLIGHT_DENY_ENV, deny.join(",")));
        }
        env
    }
}

/// Worker env carrying [`PreflightSpec::mode`].
pub const PREFLIGHT_MODE_ENV: &str = "CCTUI_PREFLIGHT_MODE";

/// Worker env carrying [`PreflightSpec::deny`], comma-separated.
pub const PREFLIGHT_DENY_ENV: &str = "CCTUI_PREFLIGHT_DENY";

/// Guard-proxy sidecar env for one profile, so a guard setting can be canaried
/// on a single profile before it reaches the global `ConfigMap`.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GuardSpec {
    /// Extra `ConfigMap`/`Secret` env sources, rendered after the global
    /// `guard-proxy-env`, so a key here overrides it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env_from: Option<Vec<EnvFromSource>>,

    /// `GUARD_PROXY_*` overrides. Container env wins over every `envFrom`
    /// source, the global one included.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<EnvVar>>,

    /// Required for [`env`](Self::env) to widen private egress
    /// (`GUARD_PROXY_ALLOW_PRIVATE_IPS` truthy, a non-empty
    /// `GUARD_PROXY_PRIVATE_ALLOW`), so the widening is explicit in review.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub allow_private_egress: bool,
}

impl WorkerProfileSpec {
    /// Effective name of the container the webhook sandboxes: the explicit
    /// [`worker_container`](Self::worker_container) override, else
    /// [`DEFAULT_WORKER_CONTAINER`].
    #[must_use]
    pub fn worker_container_name(&self) -> &str {
        self.worker_container.as_deref().unwrap_or(DEFAULT_WORKER_CONTAINER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kube::CustomResourceExt;

    #[test]
    fn crd_generates_expected_identity() {
        let crd = WorkerProfile::crd();
        assert_eq!(crd.spec.group, "cctui.dev");
        assert_eq!(crd.spec.names.kind, "WorkerProfile");
        assert_eq!(crd.spec.names.plural, "workerprofiles");
        assert_eq!(crd.spec.scope, "Namespaced", "profiles are namespaced, GitOps-managed");
        assert_eq!(crd.spec.versions.len(), 1);
        assert_eq!(crd.spec.versions[0].name, "v1alpha1");
        let yaml = serde_norway::to_string(&crd).expect("CRD serializes to YAML");
        assert!(yaml.contains("kind: CustomResourceDefinition"));
        assert!(yaml.contains("cctui.dev"));
    }

    #[test]
    fn lean_profile_deserializes() {
        let yaml = r"
image: registry.example.com/worker:latest
serviceAccountName: worker-lean
";
        let spec: WorkerProfileSpec = serde_norway::from_str(yaml).expect("lean profile parses");
        assert_eq!(spec.image, "registry.example.com/worker:latest");
        assert_eq!(spec.service_account_name.as_deref(), Some("worker-lean"));
        assert_eq!(spec.worker_container_name(), DEFAULT_WORKER_CONTAINER);
        assert!(!spec.gpg_signing);
        assert!(spec.containers.is_none());
    }

    #[test]
    fn full_stack_profile_deserializes() {
        let yaml = r#"
image: registry.example.com/worker:latest
command: ["/entrypoint"]
args: ["--serve"]
workerContainer: worker
serviceAccountName: worker-full-stack
gpgSigning: true
runtimeClassName: gvisor
resources:
  requests:
    cpu: "2"
    memory: 4Gi
  limits:
    cpu: "4"
    memory: 8Gi
env:
  - name: LOG_LEVEL
    value: info
envFrom:
  - configMapRef:
      name: worker-config
volumeMounts:
  - name: logs
    mountPath: /var/log/worker
  - name: shim
    mountPath: /usr/local/bin/shim.sh
    subPath: shim.sh
podAnnotations:
  example.dev/role: worker
nodeSelector:
  kubernetes.io/arch: amd64
imagePullSecrets:
  - name: registry-pull
volumes:
  - name: db-data
    emptyDir: {}
initContainers:
  - name: migrate
    image: registry.example.com/db-migrate:latest
containers:
  - name: db
    image: registry.example.com/postgres:16
    ports:
      - containerPort: 5432
    volumeMounts:
      - name: db-data
        mountPath: /var/lib/postgresql/data
  - name: auth-idp
    image: registry.example.com/auth-idp:latest
"#;
        let spec: WorkerProfileSpec =
            serde_norway::from_str(yaml).expect("full-stack profile parses");
        assert!(spec.gpg_signing);
        assert_eq!(spec.worker_container_name(), "worker");
        assert_eq!(spec.runtime_class_name.as_deref(), Some("gvisor"));
        let containers = spec.containers.as_ref().expect("app stack containers");
        assert_eq!(containers.len(), 2);
        assert!(containers.iter().all(|c| c.name != "worker"));
        assert_eq!(spec.init_containers.as_ref().map(Vec::len), Some(1));
        assert_eq!(spec.volumes.as_ref().map(Vec::len), Some(1));
        assert_eq!(spec.image_pull_secrets.as_ref().map(Vec::len), Some(1));
        assert_eq!(spec.env_from.as_ref().map(Vec::len), Some(1));
        assert_eq!(spec.volume_mounts.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            spec.pod_annotations
                .as_ref()
                .and_then(|a| a.get("example.dev/role"))
                .map(String::as_str),
            Some("worker")
        );
    }

    #[test]
    fn checked_in_crd_matches_the_generated_schema() {
        let checked_in: serde_json::Value =
            serde_norway::from_str(include_str!("../../../deploy/workerprofile-crd.yaml")).unwrap();
        assert_eq!(
            checked_in,
            serde_json::to_value(WorkerProfile::crd()).unwrap(),
            "deploy/workerprofile-crd.yaml is stale: regenerate it with the crdgen binary"
        );
    }

    #[test]
    fn guard_profile_deserializes() {
        let yaml = r#"
image: registry.example.com/worker:latest
guard:
  allowPrivateEgress: true
  envFrom:
    - configMapRef:
        name: guard-proxy-env-canary
  env:
    - name: GUARD_PROXY_ALLOW_PRIVATE_IPS
      value: "0"
"#;
        let spec: WorkerProfileSpec = serde_norway::from_str(yaml).expect("guard profile parses");
        let guard = spec.guard.expect("spec.guard");
        assert!(guard.allow_private_egress);
        assert_eq!(guard.env_from.as_ref().map(Vec::len), Some(1));
        assert_eq!(guard.env.as_ref().unwrap()[0].name, "GUARD_PROXY_ALLOW_PRIVATE_IPS");

        let lean: WorkerProfileSpec =
            serde_norway::from_str("image: registry.example.com/worker:latest\n").unwrap();
        assert!(lean.guard.is_none());
        assert!(serde_json::to_value(&lean).unwrap().get("guard").is_none());
    }

    #[test]
    fn preflight_profile_deserializes_into_worker_env() {
        let yaml = r#"
image: registry.example.com/worker:latest
preflight:
  mode: warn
  deny:
    - postgres.db.svc:5432
    - " 10.42.0.0/16 "
    - ""
"#;
        let spec: WorkerProfileSpec =
            serde_norway::from_str(yaml).expect("preflight profile parses");
        let env = spec.preflight.expect("spec.preflight").worker_env();
        assert_eq!(
            env,
            vec![
                (PREFLIGHT_MODE_ENV, "warn".to_owned()),
                (PREFLIGHT_DENY_ENV, "postgres.db.svc:5432,10.42.0.0/16".to_owned()),
            ]
        );

        assert!(PreflightSpec::default().worker_env().is_empty());
        let lean: WorkerProfileSpec =
            serde_norway::from_str("image: registry.example.com/worker:latest\n").unwrap();
        assert!(lean.preflight.is_none());
        assert!(serde_json::to_value(&lean).unwrap().get("preflight").is_none());
    }

    #[test]
    fn worker_container_override_honored() {
        let spec: WorkerProfileSpec = serde_norway::from_str(
            "image: registry.example.com/worker:latest\nworkerContainer: agent\n",
        )
        .unwrap();
        assert_eq!(spec.worker_container_name(), "agent");
    }
}
