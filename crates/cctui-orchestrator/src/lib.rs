//! `WorkerProfile` custom resource: the operator-authored description of a
//! workload *shape* that the dispatcher instantiates and the injection webhook
//! augments.
//!
//! # Trust model
//!
//! The profile author is **trusted** — the operator owns the cluster and manages
//! these resources via GitOps/Argo. The schema exists for ergonomics and
//! consistency, **not** for tenant isolation, so it is deliberately a thin,
//! mostly-passthrough shape close to a `PodTemplateSpec` rather than an
//! adversarial allowlist. The only adversary in the threat model is the agent
//! running inside the worker container.
//!
//! # Field ownership contract
//!
//! - **Operator-owned** (set here, in the `WorkerProfile`): everything in
//!   [`WorkerProfileSpec`]. In particular [`WorkerProfileSpec::service_account_name`]
//!   pins the identity — and therefore the IAM / secret scope — the profile runs
//!   as. A dispatch request may only select a profile *by name*; it never sets
//!   the `ServiceAccount` or any other field here.
//! - **Dispatcher-instantiated** (per run, at Job creation): the runtime env a
//!   dispatch request carries (session id, reply URL, task payload, ...) is
//!   layered onto the worker container's [`WorkerProfileSpec::env`] when the Job
//!   is created. The profile supplies the shape; the dispatcher supplies the
//!   run-specific values.
//! - **Webhook-injected** (at pod admission): the mutating webhook (CCT-726)
//!   sandboxes **only** the worker container — the secretless credential
//!   envelope, and, when [`WorkerProfileSpec::gpg_signing`] is set, the
//!   gpg-agent socket. Every other container is passthrough.
//!
//! # Identifying the worker container
//!
//! Exactly one container is the worker; the webhook and dispatcher key on it.
//! By convention it is the container named `worker`
//! ([`DEFAULT_WORKER_CONTAINER`]); a profile may override the name via
//! [`WorkerProfileSpec::worker_container`]. Use
//! [`WorkerProfileSpec::worker_container_name`] to resolve the effective name.
//! The worker container's shape comes from the first-class fields
//! ([`image`](WorkerProfileSpec::image), `command`, `args`, `resources`, `env`,
//! [`env_from`](WorkerProfileSpec::env_from),
//! [`volume_mounts`](WorkerProfileSpec::volume_mounts)); the passthrough
//! `containers` / `init_containers` carry the surrounding app stack (e.g. a
//! database or auth sidecar), which the webhook leaves untouched.

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
    /// container for remote commit signing (CCT-726). Off by default.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub gpg_signing: bool,
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
        let yaml = serde_yaml::to_string(&crd).expect("CRD serializes to YAML");
        assert!(yaml.contains("kind: CustomResourceDefinition"));
        assert!(yaml.contains("cctui.dev"));
    }

    #[test]
    fn lean_profile_deserializes() {
        let yaml = r"
image: registry.example.com/worker:latest
serviceAccountName: worker-lean
";
        let spec: WorkerProfileSpec = serde_yaml::from_str(yaml).expect("lean profile parses");
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
            serde_yaml::from_str(yaml).expect("full-stack profile parses");
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
    fn worker_container_override_honored() {
        let spec: WorkerProfileSpec = serde_yaml::from_str(
            "image: registry.example.com/worker:latest\nworkerContainer: agent\n",
        )
        .unwrap();
        assert_eq!(spec.worker_container_name(), "agent");
    }
}
