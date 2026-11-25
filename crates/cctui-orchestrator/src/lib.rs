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
    Affinity, Container, EnvFromSource, EnvVar, LocalObjectReference, ResourceRequirements,
    Toleration, Volume, VolumeMount,
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

/// Pod annotation carrying the dispatch the guard-proxy redeems its
/// service-account token for. Absent on pods no dispatcher registered.
pub const ANNOTATION_DISPATCH_ID: &str = "cctui.dev/dispatch-id";

/// Pod annotation carrying the cctui server URL the guard-proxy redeems
/// against and injects the machine key for.
pub const ANNOTATION_SERVER_URL: &str = "cctui.dev/server-url";

/// Pod annotation carrying a non-root [`Posture`]; absent means root.
pub const ANNOTATION_POSTURE: &str = "cctui.dev/posture";

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Posture {
    Root,
    Nonroot,
}

impl Posture {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Nonroot => "nonroot",
        }
    }

    /// `None` for root, so a root pod renders byte-for-byte as before.
    #[must_use]
    pub const fn annotation(self) -> Option<&'static str> {
        match self {
            Self::Root => None,
            Self::Nonroot => Some("nonroot"),
        }
    }

    #[must_use]
    pub fn from_annotation(value: Option<&str>) -> Option<Self> {
        match value {
            None | Some("root") => Some(Self::Root),
            Some("nonroot") => Some(Self::Nonroot),
            Some(_) => None,
        }
    }
}

/// Worker env that turns on exec interception; only the exact value `1` does.
pub const GUARD_EXEC_ENV: &str = "CCTUI_GUARD_EXEC";

/// Capability `cctui-guard` needs to read an intercepted exec's arguments.
///
/// It uses `pidfd_getfd` and `/proc/<pid>/mem`. Only an exec-interception
/// profile's worker may hold it.
pub const EXEC_INTERCEPTION_CAP: &str = "SYS_PTRACE";

/// Annotations on the worker pod of a `pod`-placed guard.
///
/// The guard Service it dials, the proxy token every request presents, and the
/// control token that steers the guard's policy (mounted where the agent cannot
/// read it).
pub const ANNOTATION_GUARD_SERVICE: &str = "cctui.dev/guard-service";
pub const ANNOTATION_GUARD_PROXY_TOKEN: &str = "cctui.dev/guard-proxy-token";
pub const ANNOTATION_GUARD_CONTROL_TOKEN: &str = "cctui.dev/guard-control-token";

/// Pod label (`pod`) on a worker whose guard runs as its own pod, so a static
/// `NetworkPolicy` can select those workers.
pub const LABEL_GUARD_PLACEMENT: &str = "cctui.dev/guard-placement";

/// Labels of a guard pod: [`LABEL_ROLE`] is [`ROLE_GUARD_PROXY`], and
/// [`LABEL_GUARD_FOR`] names the Job it guards.
pub const LABEL_ROLE: &str = "cctui.dev/role";
pub const ROLE_GUARD_PROXY: &str = "guard-proxy";
pub const LABEL_GUARD_FOR: &str = "cctui.dev/guard-for";

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

    /// Taints the worker pod may tolerate. Operator-owned passthrough; use
    /// together with `nodeSelector` to target a dedicated worker node group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tolerations: Option<Vec<Toleration>>,

    /// Node/pod affinity for the worker pod. Operator-owned passthrough,
    /// alongside `nodeSelector` and `tolerations`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affinity: Option<Affinity>,

    /// Annotations stamped onto every instantiated pod's template metadata (e.g.
    /// a role annotation an in-cluster mutating webhook consumes). The
    /// dispatcher's own `cctui.dev/*` session annotations win on key conflict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod_annotations: Option<BTreeMap<String, String>>,

    /// Generic pod metadata and priority class, for clusters whose admission
    /// policies require them. `None` renders the Job unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pod: Option<WorkerPodSpec>,

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

    /// Security posture of the worker container. `root` boots it as uid 0 with
    /// the envelope's capability set and the overlay warm cache; `nonroot` runs
    /// it as uid/gid 1000 with every capability dropped, no privilege
    /// escalation, `RuntimeDefault` seccomp and no overlay. Unset is `root`;
    /// new profiles set `nonroot`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub posture: Option<Posture>,

    /// Rule of Two exposure of the agent this profile runs. An unattended agent
    /// holding all three needs a [`human_gate`](Self::human_gate) when the
    /// orchestrator requires one (`CCTUI_ORCH_REQUIRE_HUMAN_GATE`); admission
    /// then refuses the profile's pods otherwise. Unset admits with an
    /// "exposure undeclared" warning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure: Option<Exposure>,

    /// The human step that bounds this profile's outward actions: `noPush`
    /// (the worker cannot push), `draftPrOnly` (it only opens draft PRs a human
    /// promotes), `outwardHeldForApproval` (outward writes wait for a human
    /// approval) or a gate the operator's guard rules define.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schemars(with = "Option<String>")]
    pub human_gate: Option<HumanGate>,

    /// Restricted DNS resolver the injected `net-init` points the pod's DNS at:
    /// an IPv4 address or a pod-search-relative name. Unset uses the
    /// orchestrator default. A resolver that does not resolve fails the pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dns_resolver: Option<String>,

    /// Resource requests/limits of the injected `net-init` and `guard-proxy`,
    /// merged key by key over the built-in defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidecar_resources: Option<SidecarResources>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct SidecarResources {
    /// Overrides for the `net-init` init container.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub net_init: Option<ResourceRequirements>,

    /// Overrides for the `guard-proxy` sidecar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_proxy: Option<ResourceRequirements>,
}

/// What the agent is exposed to, each declared explicitly.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Exposure {
    /// Reads input others wrote: ticket text, PR comments, web pages, messages.
    pub untrusted_input: bool,

    /// Reaches private repos, usable credentials or internal services.
    pub sensitive_data: bool,

    /// Acts outward: push, PR/issue comments, chat posts, any shared write.
    pub outward: bool,
}

impl Exposure {
    #[must_use]
    pub const fn is_full(self) -> bool {
        self.untrusted_input && self.sensitive_data && self.outward
    }
}

const GATE_NAME_MAX: usize = 63;

/// A gate name, stored inline so profiles stay `Copy`-friendly.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct HumanGate {
    len: usize,
    name: [u8; GATE_NAME_MAX],
}

impl HumanGate {
    pub const NO_PUSH: Self = Self::literal("noPush");
    pub const DRAFT_PR_ONLY: Self = Self::literal("draftPrOnly");
    pub const OUTWARD_HELD_FOR_APPROVAL: Self = Self::literal("outwardHeldForApproval");
    /// The gates every guard-proxy knows without a rules file.
    pub const BUILT_IN: [Self; 3] =
        [Self::NO_PUSH, Self::DRAFT_PR_ONLY, Self::OUTWARD_HELD_FOR_APPROVAL];

    const fn literal(name: &str) -> Self {
        match Self::new(name) {
            Some(gate) => gate,
            None => panic!("built-in gate name does not hold"),
        }
    }

    /// `None` unless `name` is 1 to 63 ASCII letters, digits, `-`, `_` or `.`.
    #[must_use]
    pub const fn new(name: &str) -> Option<Self> {
        let bytes = name.as_bytes();
        if bytes.is_empty() || bytes.len() > GATE_NAME_MAX {
            return None;
        }
        let mut out = [0u8; GATE_NAME_MAX];
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if !(b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
                return None;
            }
            out[i] = b;
            i += 1;
        }
        Some(Self { len: bytes.len(), name: out })
    }

    /// The profile spelling, which the guard-proxy reads from
    /// `GUARD_PROXY_HUMAN_GATE`.
    #[must_use]
    pub const fn as_str(&self) -> &str {
        match std::str::from_utf8(self.name.split_at(self.len).0) {
            Ok(name) => name,
            Err(_) => "",
        }
    }
}

impl std::fmt::Debug for HumanGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("HumanGate").field(&self.as_str()).finish()
    }
}

impl Serialize for HumanGate {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for HumanGate {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Self::new(&name).ok_or_else(|| {
            serde::de::Error::custom(format!(
                "humanGate `{name}`: want 1 to {GATE_NAME_MAX} ASCII letters, digits, `-`, `_` \
                 or `.`"
            ))
        })
    }
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

    /// Cloud workload identity (e.g. EKS Pod Identity) wired into the
    /// guard-proxy alone. Admission denies the pod if any other container
    /// mounts one of these volumes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_identity: Option<CloudIdentitySpec>,

    /// `sidecar` (unset) runs the guard-proxy beside the worker behind
    /// `net-init`; `pod` runs it as a per-dispatch pod the worker reaches
    /// through a Service, with no `net-init` and no `NET_ADMIN`. `pod` needs
    /// `posture: nonroot`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub placement: Option<GuardPlacement>,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum GuardPlacement {
    #[default]
    Sidecar,
    Pod,
}

/// The guard-proxy's share of a cloud identity webhook's injection.
///
/// The webhook must be told to skip every other container (e.g.
/// `eks.amazonaws.com/skip-containers` in `podAnnotations`). The guard-proxy
/// gets these mounts and env from cctui itself, so it holds the identity
/// whichever webhook runs first.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct CloudIdentitySpec {
    /// Mounts of the volumes the identity webhook adds to the pod. Each named
    /// volume must be a projected service-account token or a read-only CSI
    /// volume, and only the guard-proxy may mount it.
    pub volume_mounts: Vec<VolumeMount>,

    /// Literal env the cloud SDK reads to find the credentials.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<EnvVar>>,
}

impl GuardSpec {
    /// Names of the cloud identity volumes only the guard-proxy may mount.
    pub fn identity_volumes(&self) -> impl Iterator<Item = &str> {
        self.cloud_identity.iter().flat_map(|c| &c.volume_mounts).map(|m| m.name.as_str())
    }
}

/// Label and annotation key prefixes owned by cctui or the Job controller.
pub const RESERVED_POD_KEY_PREFIXES: [&str; 2] = ["cctui.dev/", "batch.kubernetes.io/"];

/// Legacy Job-controller selector labels, reserved on top of the prefixes.
pub const RESERVED_POD_LABELS: [&str; 2] = ["controller-uid", "job-name"];

/// Generic metadata and scheduling class stamped onto the worker Job and its
/// pod template.
#[derive(Serialize, Deserialize, Debug, Clone, Default, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct WorkerPodSpec {
    /// Labels for the worker Job and its pod template. Keys cctui or the Job
    /// controller manage (`cctui.dev/*`, `batch.kubernetes.io/*`, `job-name`,
    /// `controller-uid`) are denied at admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<BTreeMap<String, String>>,

    /// Annotations for the worker Job and its pod template. `cctui.dev/*` and
    /// `batch.kubernetes.io/*` keys are denied at admission.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<BTreeMap<String, String>>,

    /// `PriorityClass` of the worker pod.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority_class_name: Option<String>,
}

impl WorkerProfileSpec {
    /// Effective name of the container the webhook sandboxes: the explicit
    /// [`worker_container`](Self::worker_container) override, else
    /// [`DEFAULT_WORKER_CONTAINER`].
    #[must_use]
    pub fn worker_container_name(&self) -> &str {
        self.worker_container.as_deref().unwrap_or(DEFAULT_WORKER_CONTAINER)
    }

    #[must_use]
    pub fn posture(&self) -> Posture {
        self.posture.unwrap_or(Posture::Root)
    }

    #[must_use]
    pub fn guard_placement(&self) -> GuardPlacement {
        self.guard.as_ref().and_then(|g| g.placement).unwrap_or_default()
    }

    /// Whether the profile opts into exec interception.
    ///
    /// Its own `env` must set [`GUARD_EXEC_ENV`] to the literal `1`.
    #[must_use]
    pub fn exec_interception(&self) -> bool {
        self.env
            .iter()
            .flatten()
            .any(|e| e.name == GUARD_EXEC_ENV && e.value.as_deref() == Some("1"))
    }

    #[must_use]
    pub fn pod_labels(&self) -> Option<&BTreeMap<String, String>> {
        self.pod.as_ref()?.labels.as_ref()
    }

    #[must_use]
    pub fn pod_metadata_annotations(&self) -> Option<&BTreeMap<String, String>> {
        self.pod.as_ref()?.annotations.as_ref()
    }

    #[must_use]
    pub fn priority_class_name(&self) -> Option<&str> {
        self.pod.as_ref()?.priority_class_name.as_deref()
    }

    #[must_use]
    pub fn dns_resolver(&self) -> Option<&str> {
        self.dns_resolver.as_deref().map(str::trim).filter(|r| !r.is_empty())
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
    fn dedicated_node_example_round_trips_tolerations() {
        let profile: WorkerProfile = serde_norway::from_str(include_str!(
            "../../../deploy/examples/core-kubernetes/workerprofile.yaml"
        ))
        .unwrap();
        let spec = serde_json::to_value(&profile.spec).unwrap();
        assert_eq!(spec["tolerations"][0]["key"], "dedicated");
        assert_eq!(spec["tolerations"][0]["value"], "cctui-worker");
        assert_eq!(spec["tolerations"][1]["tolerationSeconds"], 30);
        assert_eq!(spec["posture"], "nonroot");
        let schema = serde_json::to_value(WorkerProfile::crd()).unwrap();
        let tolerations = &schema["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]
            ["spec"]["properties"]["tolerations"];
        assert_eq!(tolerations["type"], "array");
        assert_eq!(tolerations["items"]["properties"]["tolerationSeconds"]["type"], "integer");
        assert!(tolerations.get("default").is_none());
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
    fn guard_placement_defaults_to_the_sidecar_and_parses_pod() {
        let lean: WorkerProfileSpec =
            serde_norway::from_str("image: registry.example.com/worker:latest\n").unwrap();
        assert_eq!(lean.guard_placement(), GuardPlacement::Sidecar);
        let pod: WorkerProfileSpec = serde_norway::from_str(
            "image: registry.example.com/worker:latest\nguard:\n  placement: pod\n",
        )
        .unwrap();
        assert_eq!(pod.guard_placement(), GuardPlacement::Pod);
        assert_eq!(serde_json::to_value(&pod).unwrap()["guard"]["placement"], "pod");
        let unset = serde_json::to_value(GuardSpec::default()).unwrap();
        assert!(unset.get("placement").is_none());
        assert!(
            serde_norway::from_str::<WorkerProfileSpec>(
                "image: registry.example.com/worker:latest\nguard:\n  placement: node\n"
            )
            .is_err()
        );
    }

    #[test]
    fn guard_cloud_identity_deserializes() {
        let yaml = r"
image: registry.example.com/worker:latest
guard:
  cloudIdentity:
    volumeMounts:
      - name: eks-pod-identity-token
        mountPath: /var/run/secrets/pods.eks.amazonaws.com/serviceaccount
        readOnly: true
    env:
      - name: AWS_CONTAINER_CREDENTIALS_FULL_URI
        value: http://169.254.170.23/v1/credentials
";
        let guard = serde_norway::from_str::<WorkerProfileSpec>(yaml).unwrap().guard.unwrap();
        assert_eq!(guard.identity_volumes().collect::<Vec<_>>(), ["eks-pod-identity-token"]);
        assert_eq!(guard.cloud_identity.unwrap().env.unwrap().len(), 1);
        assert_eq!(GuardSpec::default().identity_volumes().count(), 0);
    }

    #[test]
    fn pod_settings_and_affinity_deserialize_and_are_in_the_crd_schema() {
        let yaml = r#"
image: registry.example.com/worker:latest
affinity:
  nodeAffinity:
    requiredDuringSchedulingIgnoredDuringExecution:
      nodeSelectorTerms:
        - matchExpressions:
            - key: pool
              operator: In
              values: ["workers"]
pod:
  labels:
    team: platform
  annotations:
    policy.example.dev/owner: platform
  priorityClassName: cctui-workers
"#;
        let spec: WorkerProfileSpec = serde_norway::from_str(yaml).expect("pod profile parses");
        assert_eq!(spec.pod_labels().unwrap()["team"], "platform");
        assert_eq!(
            spec.pod_metadata_annotations().unwrap()["policy.example.dev/owner"],
            "platform"
        );
        assert_eq!(spec.priority_class_name(), Some("cctui-workers"));
        let node = spec.affinity.as_ref().and_then(|a| a.node_affinity.as_ref()).unwrap();
        let required = node.required_during_scheduling_ignored_during_execution.as_ref().unwrap();
        let expr = &required.node_selector_terms[0].match_expressions.as_ref().unwrap()[0];
        assert_eq!(expr.key, "pool");

        let lean: WorkerProfileSpec =
            serde_norway::from_str("image: registry.example.com/worker:latest\n").unwrap();
        assert!(lean.pod.is_none() && lean.affinity.is_none());
        assert!(lean.pod_labels().is_none() && lean.priority_class_name().is_none());
        let lean_json = serde_json::to_value(&lean).unwrap();
        assert!(lean_json.get("pod").is_none() && lean_json.get("affinity").is_none());

        let crd = serde_json::to_value(WorkerProfile::crd()).unwrap();
        let props = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]
            ["properties"];
        let pod = &props["pod"]["properties"];
        assert_eq!(pod["labels"]["additionalProperties"]["type"], "string");
        assert_eq!(pod["annotations"]["additionalProperties"]["type"], "string");
        assert_eq!(pod["priorityClassName"]["type"], "string");
        let affinity = &props["affinity"]["properties"];
        for key in ["nodeAffinity", "podAffinity", "podAntiAffinity"] {
            assert_eq!(affinity[key]["type"], "object", "{key}");
        }
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
    fn posture_deserializes_and_unset_is_root() {
        let lean: WorkerProfileSpec =
            serde_norway::from_str("image: registry.example.com/worker:latest\n").unwrap();
        assert!(lean.posture.is_none());
        assert_eq!(lean.posture(), Posture::Root);
        assert!(serde_json::to_value(&lean).unwrap().get("posture").is_none());

        for (yaml, want) in [("root", Posture::Root), ("nonroot", Posture::Nonroot)] {
            let spec: WorkerProfileSpec = serde_norway::from_str(&format!(
                "image: registry.example.com/worker:latest\nposture: {yaml}\n"
            ))
            .unwrap();
            assert_eq!(spec.posture(), want);
            assert_eq!(serde_json::to_value(&spec).unwrap()["posture"], yaml);
        }
        assert!(
            serde_norway::from_str::<WorkerProfileSpec>(
                "image: registry.example.com/worker:latest\nposture: restricted\n"
            )
            .is_err()
        );
    }

    #[test]
    fn crd_posture_has_no_schema_default() {
        let crd = serde_json::to_value(WorkerProfile::crd()).unwrap();
        let posture = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]
            ["properties"]["posture"];
        let values = posture["enum"].as_array().expect("posture is an enum");
        for want in ["root", "nonroot"] {
            assert!(values.contains(&serde_json::json!(want)), "{want} missing from {values:?}");
        }
        assert!(
            posture.get("default").is_none(),
            "a schema default would be applied to stored profiles on read and flip them"
        );
    }

    #[test]
    fn exposure_and_human_gate_deserialize_and_unset_stays_unset() {
        let spec: WorkerProfileSpec = serde_norway::from_str(
            "image: registry.example.com/worker:latest\n\
             exposure:\n  untrustedInput: true\n  sensitiveData: true\n  outward: false\n\
             humanGate: draftPrOnly\n",
        )
        .unwrap();
        let exposure = spec.exposure.unwrap();
        assert!(exposure.untrusted_input && exposure.sensitive_data && !exposure.outward);
        assert!(!exposure.is_full());
        assert_eq!(spec.human_gate, Some(HumanGate::DRAFT_PR_ONLY));
        let json = serde_json::to_value(&spec).unwrap();
        assert_eq!(json["exposure"]["untrustedInput"], true);
        assert_eq!(json["humanGate"], "draftPrOnly");

        let lean: WorkerProfileSpec =
            serde_norway::from_str("image: registry.example.com/worker:latest\n").unwrap();
        assert!(lean.exposure.is_none() && lean.human_gate.is_none());
        let lean_json = serde_json::to_value(&lean).unwrap();
        assert!(lean_json.get("exposure").is_none() && lean_json.get("humanGate").is_none());

        assert!(
            serde_norway::from_str::<WorkerProfileSpec>(
                "image: registry.example.com/worker:latest\nexposure:\n  outward: true\n"
            )
            .is_err(),
            "a declared exposure names all three"
        );
        let gate = |raw: &str| {
            serde_norway::from_str::<WorkerProfileSpec>(&format!(
                "image: registry.example.com/worker:latest\nhumanGate: {raw}\n"
            ))
            .map(|spec| spec.human_gate)
        };
        let custom = gate("reviewWithScreenshots").unwrap();
        assert_eq!(custom.as_ref().map(HumanGate::as_str), Some("reviewWithScreenshots"));
        assert_eq!(serde_json::to_value(custom).unwrap(), "reviewWithScreenshots");
        for bad in ["\"\"".to_owned(), "\"two words\"".to_owned(), "x".repeat(64)] {
            assert!(gate(&bad).is_err(), "{bad}");
        }
        assert!(gate(&"x".repeat(63)).is_ok());
    }

    #[test]
    fn crd_exposure_requires_all_three_and_names_any_gate() {
        let crd = serde_json::to_value(WorkerProfile::crd()).unwrap();
        let props = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]
            ["properties"];
        let exposure = &props["exposure"];
        assert_eq!(exposure["type"], "object");
        assert_eq!(
            exposure["required"],
            serde_json::json!(["outward", "sensitiveData", "untrustedInput"])
        );
        for key in ["untrustedInput", "sensitiveData", "outward"] {
            assert_eq!(exposure["properties"][key]["type"], "boolean", "{key}");
        }
        assert_eq!(props["humanGate"]["type"], "string");
        assert!(props["humanGate"].get("enum").is_none(), "rule sets name the gates");
        assert!(props["humanGate"].get("default").is_none());
    }

    #[test]
    fn posture_annotation_round_trips() {
        assert_eq!(Posture::Root.annotation(), None);
        assert_eq!(Posture::Nonroot.annotation(), Some("nonroot"));
        for p in [Posture::Root, Posture::Nonroot] {
            assert_eq!(Posture::from_annotation(p.annotation()), Some(p));
            assert_eq!(Posture::from_annotation(Some(p.as_str())), Some(p));
        }
        assert_eq!(Posture::from_annotation(Some("restricted")), None);
    }

    #[test]
    fn dns_resolver_and_sidecar_resources_deserialize_and_are_in_the_crd_schema() {
        let spec: WorkerProfileSpec = serde_norway::from_str(
            "image: registry.example.com/worker:latest\n\
             dnsResolver: \" cctui-worker-dns-canary \"\n\
             sidecarResources:\n  guardProxy:\n    limits:\n      ephemeral-storage: 1Gi\n",
        )
        .unwrap();
        assert_eq!(spec.dns_resolver(), Some("cctui-worker-dns-canary"));
        let sidecars = spec.sidecar_resources.as_ref().unwrap();
        assert!(sidecars.net_init.is_none());
        let limits = sidecars.guard_proxy.as_ref().unwrap().limits.as_ref().unwrap();
        assert_eq!(limits["ephemeral-storage"].0, "1Gi");

        let blank: WorkerProfileSpec = serde_norway::from_str(
            "image: registry.example.com/worker:latest\ndnsResolver: \"  \"\n",
        )
        .unwrap();
        assert_eq!(blank.dns_resolver(), None);
        let lean: WorkerProfileSpec =
            serde_norway::from_str("image: registry.example.com/worker:latest\n").unwrap();
        assert!(lean.dns_resolver().is_none() && lean.sidecar_resources.is_none());
        let lean_json = serde_json::to_value(&lean).unwrap();
        assert!(lean_json.get("dnsResolver").is_none());
        assert!(lean_json.get("sidecarResources").is_none());

        let crd = serde_json::to_value(WorkerProfile::crd()).unwrap();
        let props = &crd["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"]
            ["properties"];
        assert_eq!(props["dnsResolver"]["type"], "string");
        let sidecars = &props["sidecarResources"]["properties"];
        for key in ["netInit", "guardProxy"] {
            assert_eq!(sidecars[key]["properties"]["limits"]["type"], "object", "{key}");
            assert_eq!(sidecars[key]["properties"]["requests"]["type"], "object", "{key}");
        }
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
