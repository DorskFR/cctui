//! Pure construction of the secretless-worker envelope injected into profiled
//! pods, plus the [`mutate_pod`] entry point that turns an admitted pod into a
//! `JSONPatch`.
//!
//! Only the worker container is sandboxed; every other container and
//! initContainer is passthrough (threat model: the operator is trusted, the
//! agent in the worker container is not). Environment-specific guard-proxy
//! settings are supplied at runtime via the `guard-proxy-env` `ConfigMap`
//! (`envFrom`), never baked into the binary; a profile's `spec.guard` layers
//! on top of it.

use crate::{
    ANNOTATION_DISPATCH_ID, ANNOTATION_ENVELOPE_INJECTED, ANNOTATION_GPG_SIGNING,
    ANNOTATION_GUARD_CONTROL_TOKEN, ANNOTATION_GUARD_IDENTITY, ANNOTATION_GUARD_PROXY_TOKEN,
    ANNOTATION_GUARD_SERVICE, ANNOTATION_POSTURE, ANNOTATION_SERVER_URL,
    ANNOTATION_WORKER_CONTAINER, DEFAULT_WORKER_CONTAINER, EXEC_INTERCEPTION_CAP, GuardPlacement,
    GuardSpec, HumanGate, LABEL_GUARD_FOR, LABEL_ROLE, LABEL_WORKER_PROFILE, Posture,
    ROLE_GUARD_PROXY, SidecarResources, WorkerProfileSpec,
};
use k8s_openapi::api::core::v1::{
    AppArmorProfile, Capabilities, ConfigMapEnvSource, ConfigMapProjection, ConfigMapVolumeSource,
    Container, ContainerPort, DownwardAPIProjection, DownwardAPIVolumeFile,
    DownwardAPIVolumeSource, EmptyDirVolumeSource, EnvFromSource, EnvVar, EnvVarSource,
    HTTPGetAction, KeyToPath, ObjectFieldSelector, Pod, PodSecurityContext, PodSpec, Probe,
    ProjectedVolumeSource, ResourceRequirements, SeccompProfile, SecurityContext, Service,
    ServiceAccountTokenProjection, ServicePort, ServiceSpec, Volume, VolumeMount, VolumeProjection,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{ObjectMeta, OwnerReference};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::BTreeMap;

pub const NET_INIT_CONTAINER: &str = "net-init";
pub const GUARD_PROXY_CONTAINER: &str = "guard-proxy";
const CONFIGMAP_GUARD_PROXY_ENV: &str = "guard-proxy-env";
const CONFIGMAP_GUARD_PROXY_INJECT: &str = "guard-proxy-inject";
pub const GUARD_ENV_HUMAN_GATE: &str = "GUARD_PROXY_HUMAN_GATE";
pub const NET_INIT_ENV_DNS_RESOLVER: &str = "WORKER_DNS_RESOLVER";
/// Overridable at runtime via `CCTUI_ORCH_DNS_RESOLVER`, per profile via
/// `spec.dnsResolver`.
pub const DEFAULT_DNS_RESOLVER: &str = "cctui-worker-dns";

/// `(resource, request, limit)` defaults, overridable via `spec.sidecarResources`.
pub const NET_INIT_RESOURCES: [(&str, &str, &str); 3] =
    [("cpu", "10m", "100m"), ("memory", "16Mi", "64Mi"), ("ephemeral-storage", "4Mi", "16Mi")];
pub const GUARD_PROXY_RESOURCES: [(&str, &str, &str); 3] =
    [("cpu", "25m", "500m"), ("memory", "32Mi", "256Mi"), ("ephemeral-storage", "32Mi", "128Mi")];

pub const VOL_HOME: &str = "home";
pub const VOL_OVERLAY: &str = "overlay";
pub const VOL_GUARD_STATE: &str = "guard-state";
pub const VOL_PROXY_POLICY: &str = "proxy-policy";
pub const VOL_GUARD_PROXY_INJECT: &str = "guard-proxy-inject";
pub const VOL_GUARD_PROXY_CA: &str = "guard-proxy-ca";
pub const VOL_GUARD_PROXY_SA_TOKEN: &str = "guard-proxy-sa-token";
pub const VOL_GPG_AGENT: &str = "gpg-agent";
pub const VOL_GUARD_AUTH: &str = "guard-auth";

/// Volumes, and worker mounts, of a worker whose guard runs as its own pod.
pub const EXTERNAL_GUARD_WORKER_VOLUMES: [&str; 5] =
    [VOL_HOME, VOL_GUARD_STATE, VOL_PROXY_POLICY, VOL_GUARD_PROXY_CA, VOL_GUARD_AUTH];
const GUARD_AUTH_DIR: &str = "/var/run/guard-auth";
pub const GUARD_PROXY_TOKEN_FILE: &str = "proxy-token";
pub const GUARD_CONTROL_TOKEN_FILE: &str = "control-token";
pub const WORKER_NET_MODE_EXTERNAL_POD: &str = "external-pod";
pub const GUARD_PORT_PROXY: i32 = 15001;
pub const GUARD_PORT_CONTROL: i32 = 15002;
pub const GUARD_PORT_GATEWAY: i32 = 15003;

/// Where the kubelet would mount the pod's service-account token. Pods set
/// `automountServiceAccountToken: false`; only the guard-proxy (Vault
/// Kubernetes login, `k8s:` secret refs) gets an equivalent projection.
const SA_TOKEN_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
const SA_TOKEN_EXPIRATION_SECONDS: i64 = 3600;
/// The token the guard-proxy redeems once at startup, so the shortest lifetime
/// the API server accepts.
const CCTUI_TOKEN_EXPIRATION_SECONDS: i64 = 600;
pub const CCTUI_TOKEN_FILE: &str = "cctui-token";
pub const CCTUI_TOKEN_AUDIENCE: &str = "cctui";
const CONFIGMAP_KUBE_ROOT_CA: &str = "kube-root-ca.crt";

/// Volume names the envelope mounts into the worker container under `root`.
/// Mirrors [`sandbox_worker`]; see [`worker_envelope_mounts`].
pub const WORKER_ENVELOPE_MOUNTS: [&str; 5] =
    [VOL_HOME, VOL_OVERLAY, VOL_GUARD_STATE, VOL_PROXY_POLICY, VOL_GUARD_PROXY_CA];

/// Points the entrypoint at an overlayfs warm cache, which needs `SYS_ADMIN`.
pub const WARM_REPO_DIR_ENV: &str = "WARM_REPO_DIR";

pub const PROXY_UID: i64 = 1337;
pub const WORKER_UID: i64 = 1000;
const FS_GROUP: i64 = WORKER_UID;

pub const WORKER_ADDED_CAPS: [&str; 9] = [
    "SYS_ADMIN",
    "CHOWN",
    "DAC_OVERRIDE",
    "FOWNER",
    "FSETID",
    "KILL",
    "SETUID",
    "SETGID",
    "SETPCAP",
];

/// Default sidecar/init image, pinned to this binary's version. Overridable at
/// runtime via `CCTUI_ORCH_SIDECAR_IMAGE`.
#[must_use]
pub fn default_sidecar_image() -> String {
    format!("ghcr.io/dorskfr/cctui-worker:{}", env!("CARGO_PKG_VERSION"))
}

#[must_use]
pub const fn has_overlay(posture: Posture) -> bool {
    matches!(posture, Posture::Root)
}

/// Volume names the envelope mounts into the worker container;
/// [`VOL_GPG_AGENT`] is added on top only under `gpgSigning`.
#[must_use]
pub fn worker_envelope_mounts(posture: Posture) -> Vec<&'static str> {
    WORKER_ENVELOPE_MOUNTS
        .into_iter()
        .filter(|m| has_overlay(posture) || *m != VOL_OVERLAY)
        .collect()
}

#[must_use]
pub const fn worker_added_caps(posture: Posture) -> &'static [&'static str] {
    match posture {
        Posture::Root => &WORKER_ADDED_CAPS,
        Posture::Nonroot => &[],
    }
}

fn restricted(uid: i64) -> SecurityContext {
    SecurityContext {
        run_as_user: Some(uid),
        run_as_group: Some(uid),
        run_as_non_root: Some(true),
        allow_privilege_escalation: Some(false),
        capabilities: Some(Capabilities { drop: Some(vec!["ALL".to_owned()]), add: None }),
        seccomp_profile: Some(SeccompProfile {
            type_: "RuntimeDefault".to_owned(),
            localhost_profile: None,
        }),
        ..SecurityContext::default()
    }
}

#[must_use]
pub fn worker_security_context(posture: Posture) -> SecurityContext {
    worker_security_context_with(posture, false)
}

/// [`worker_security_context`], optionally with [`EXEC_INTERCEPTION_CAP`].
///
/// The root set gains it when `exec_interception`; nonroot never does.
#[must_use]
pub fn worker_security_context_with(posture: Posture, exec_interception: bool) -> SecurityContext {
    if posture == Posture::Nonroot {
        return restricted(WORKER_UID);
    }
    let mut add: Vec<String> = WORKER_ADDED_CAPS.iter().map(|s| (*s).to_owned()).collect();
    if exec_interception {
        add.push(EXEC_INTERCEPTION_CAP.to_owned());
    }
    SecurityContext {
        run_as_user: Some(0),
        app_armor_profile: Some(AppArmorProfile {
            type_: "Unconfined".to_owned(),
            localhost_profile: None,
        }),
        capabilities: Some(Capabilities { drop: Some(vec!["ALL".to_owned()]), add: Some(add) }),
        ..SecurityContext::default()
    }
}

fn guard_proxy_security_context(posture: Posture) -> SecurityContext {
    match posture {
        Posture::Root => SecurityContext {
            run_as_user: Some(PROXY_UID),
            run_as_group: Some(PROXY_UID),
            allow_privilege_escalation: Some(false),
            capabilities: Some(Capabilities { drop: Some(vec!["ALL".to_owned()]), add: None }),
            ..SecurityContext::default()
        },
        Posture::Nonroot => restricted(PROXY_UID),
    }
}

/// Everything the envelope construction needs, resolved from the pod's
/// admission-time label/annotations.
pub struct EnvelopeConfig {
    pub sidecar_image: String,
    pub worker_container: String,
    pub guard_identity: Option<String>,
    pub gpg: bool,
    pub guard: Option<GuardSpec>,
    pub posture: Posture,
    pub dispatched: bool,
    /// See [`crate::WorkerProfileSpec::exec_interception`].
    pub exec_interception: bool,
    /// See [`crate::WorkerProfileSpec::human_gate`].
    pub human_gate: Option<HumanGate>,
    pub dns_resolver: String,
    pub sidecar_resources: Option<SidecarResources>,
}

/// What the source profile contributes to the envelope besides `spec.guard`.
#[derive(Debug, Clone, Default)]
pub struct ProfileFlags {
    /// See [`crate::WorkerProfileSpec::exec_interception`].
    pub exec_interception: bool,
    /// See [`crate::WorkerProfileSpec::human_gate`].
    pub human_gate: Option<HumanGate>,
    /// Unset or blank renders [`DEFAULT_DNS_RESOLVER`].
    pub dns_resolver: Option<String>,
    pub sidecar_resources: Option<SidecarResources>,
}

impl EnvelopeConfig {
    fn guard_pod(&self) -> bool {
        self.guard.as_ref().and_then(|g| g.placement) == Some(GuardPlacement::Pod)
    }

    fn from_pod(
        pod: &Pod,
        sidecar_image: &str,
        guard: Option<&GuardSpec>,
        flags: ProfileFlags,
    ) -> anyhow::Result<Self> {
        let annotations = pod.metadata.annotations.as_ref();
        let get = |key: &str| annotations.and_then(|a| a.get(key)).map(String::as_str);
        let posture = Posture::from_annotation(get(ANNOTATION_POSTURE)).ok_or_else(|| {
            anyhow::anyhow!(
                "pod annotation `{ANNOTATION_POSTURE}: {}` names no posture (want `nonroot`, or \
                 no annotation for `root`)",
                get(ANNOTATION_POSTURE).unwrap_or_default()
            )
        })?;
        let dns_resolver = flags.dns_resolver.as_deref().map(str::trim).filter(|r| !r.is_empty());
        Ok(Self {
            sidecar_image: sidecar_image.to_owned(),
            worker_container: get(ANNOTATION_WORKER_CONTAINER)
                .unwrap_or(DEFAULT_WORKER_CONTAINER)
                .to_owned(),
            guard_identity: get(ANNOTATION_GUARD_IDENTITY).map(ToOwned::to_owned),
            gpg: get(ANNOTATION_GPG_SIGNING) == Some("true"),
            guard: guard.cloned(),
            posture,
            dispatched: get(ANNOTATION_DISPATCH_ID).is_some(),
            exec_interception: flags.exec_interception,
            human_gate: flags.human_gate,
            dns_resolver: dns_resolver.unwrap_or(DEFAULT_DNS_RESOLVER).to_owned(),
            sidecar_resources: flags.sidecar_resources,
        })
    }
}

/// A pod is eligible when it carries the worker-profile label.
#[must_use]
pub fn is_eligible(pod: &Pod) -> bool {
    pod.metadata.labels.as_ref().is_some_and(|l| l.contains_key(LABEL_WORKER_PROFILE))
}

/// Already-injected pods are detected by the marker annotation or an existing
/// guard-proxy sidecar, so re-invocation is a no-op.
#[must_use]
pub fn is_injected(pod: &Pod) -> bool {
    let marked =
        pod.metadata.annotations.as_ref().is_some_and(|a| {
            a.get(ANNOTATION_ENVELOPE_INJECTED).map(String::as_str) == Some("true")
        });
    let has_sidecar = pod
        .spec
        .as_ref()
        .and_then(|s| s.init_containers.as_ref())
        .is_some_and(|inits| inits.iter().any(|c| c.name == GUARD_PROXY_CONTAINER));
    marked || has_sidecar
}

#[derive(Debug)]
pub enum MutationOutcome {
    NotEligible,
    AlreadyInjected,
    Patch(json_patch::Patch),
}

/// Profile name from the worker-profile label, if the pod carries one.
#[must_use]
pub fn profile_name(pod: &Pod) -> Option<&str> {
    pod.metadata.labels.as_ref().and_then(|l| l.get(LABEL_WORKER_PROFILE)).map(String::as_str)
}

/// Evaluate `pod` for envelope injection.
///
/// `guard` is the source profile's `spec.guard`, `flags` the rest it renders
/// into the envelope. A serialization failure or an
/// unknown posture annotation is an `Err` so the admission webhook fails closed
/// (deny) instead of admitting an unpatched, unsandboxed worker.
pub fn mutate_pod(
    pod: &Pod,
    sidecar_image: &str,
    guard: Option<&GuardSpec>,
    flags: ProfileFlags,
) -> anyhow::Result<MutationOutcome> {
    if !is_eligible(pod) {
        return Ok(MutationOutcome::NotEligible);
    }
    if is_injected(pod) {
        return Ok(MutationOutcome::AlreadyInjected);
    }
    let cfg = EnvelopeConfig::from_pod(pod, sidecar_image, guard, flags)?;
    let injected = inject(pod, &cfg);
    let before =
        serde_json::to_value(pod).map_err(|e| anyhow::anyhow!("serializing admitted pod: {e}"))?;
    let after = serde_json::to_value(&injected)
        .map_err(|e| anyhow::anyhow!("serializing injected pod: {e}"))?;
    let patch = json_patch::diff(&before, &after);
    if patch.0.is_empty() {
        Ok(MutationOutcome::NotEligible)
    } else {
        Ok(MutationOutcome::Patch(patch))
    }
}

/// Return a clone of `pod` with the envelope injected. Pure: only the worker
/// container is mutated; all other containers/initContainers are untouched.
#[must_use]
pub fn inject(pod: &Pod, cfg: &EnvelopeConfig) -> Pod {
    if cfg.guard_pod() {
        return inject_external_guard(pod, cfg);
    }
    let mut pod = pod.clone();
    let spec = pod.spec.get_or_insert_with(Default::default);

    let init = spec.init_containers.get_or_insert_with(Vec::new);
    init.insert(0, net_init_container(cfg));
    init.insert(1, guard_proxy_container(cfg));

    let pod_sec = spec.security_context.get_or_insert_with(PodSecurityContext::default);
    if pod_sec.fs_group.is_none() {
        pod_sec.fs_group = Some(FS_GROUP);
    }

    let volumes = spec.volumes.get_or_insert_with(Vec::new);
    for vol in envelope_volumes(cfg) {
        if !volumes.iter().any(|v| v.name == vol.name) {
            volumes.push(vol);
        }
    }

    if let Some(worker) = spec.containers.iter_mut().find(|c| c.name == cfg.worker_container) {
        sandbox_worker(worker, cfg);
    }

    pod.metadata
        .annotations
        .get_or_insert_with(BTreeMap::new)
        .insert(ANNOTATION_ENVELOPE_INJECTED.to_owned(), "true".to_owned());

    pod
}

fn sandbox_worker(worker: &mut Container, cfg: &EnvelopeConfig) {
    worker.security_context =
        Some(worker_security_context_with(cfg.posture, cfg.exec_interception));

    let env = worker.env.get_or_insert_with(Vec::new);
    if !has_overlay(cfg.posture) {
        env.retain(|e| e.name != WARM_REPO_DIR_ENV);
    }
    upsert_env(env, "WORKER_NET_MODE", "transparent-external");
    upsert_env(env, "NODE_EXTRA_CA_CERTS", "/var/run/guard-proxy-ca/ca.pem");

    let mounts = worker.volume_mounts.get_or_insert_with(Vec::new);
    let mut wanted = vec![
        mount(VOL_HOME, "/home/worker", false),
        mount(VOL_OVERLAY, "/overlay", false),
        mount(VOL_GUARD_STATE, "/var/run/workflow-guard", false),
        mount(VOL_PROXY_POLICY, "/var/run/guard-proxy", false),
        mount(VOL_GUARD_PROXY_CA, "/var/run/guard-proxy-ca", false),
    ];
    if cfg.gpg {
        wanted.push(mount(VOL_GPG_AGENT, "/var/run/gpg-agent", false));
    }
    wanted.retain(|m| has_overlay(cfg.posture) || m.name != VOL_OVERLAY);
    for m in wanted {
        if !mounts.iter().any(|x| x.name == m.name) {
            mounts.push(m);
        }
    }
}

/// The worker half of a `pod`-placed guard: no `net-init`, no sidecar, the
/// restricted worker, and the guard reached through its Service.
fn inject_external_guard(pod: &Pod, cfg: &EnvelopeConfig) -> Pod {
    let mut pod = pod.clone();
    let spec = pod.spec.get_or_insert_with(Default::default);
    let pod_sec = spec.security_context.get_or_insert_with(PodSecurityContext::default);
    if pod_sec.fs_group.is_none() {
        pod_sec.fs_group = Some(FS_GROUP);
    }
    let volumes = spec.volumes.get_or_insert_with(Vec::new);
    let wanted = vec![
        empty_dir(VOL_HOME),
        empty_dir(VOL_GUARD_STATE),
        empty_dir(VOL_PROXY_POLICY),
        empty_dir(VOL_GUARD_PROXY_CA),
        guard_auth_volume(),
    ];
    for vol in wanted {
        if !volumes.iter().any(|v| v.name == vol.name) {
            volumes.push(vol);
        }
    }
    if let Some(worker) = spec.containers.iter_mut().find(|c| c.name == cfg.worker_container) {
        worker.security_context = Some(restricted(WORKER_UID));
        let env = worker.env.get_or_insert_with(Vec::new);
        env.retain(|e| e.name != WARM_REPO_DIR_ENV && e.name != "GUARD_PROXY_HOST");
        upsert_env(env, "WORKER_NET_MODE", WORKER_NET_MODE_EXTERNAL_POD);
        upsert_env(env, "NODE_EXTRA_CA_CERTS", "/var/run/guard-proxy-ca/ca.pem");
        env.push(annotation_env("GUARD_PROXY_HOST", ANNOTATION_GUARD_SERVICE));
        let mounts = worker.volume_mounts.get_or_insert_with(Vec::new);
        for m in [
            mount(VOL_HOME, "/home/worker", false),
            mount(VOL_GUARD_STATE, "/var/run/workflow-guard", false),
            mount(VOL_PROXY_POLICY, "/var/run/guard-proxy", false),
            mount(VOL_GUARD_PROXY_CA, "/var/run/guard-proxy-ca", false),
            mount(VOL_GUARD_AUTH, GUARD_AUTH_DIR, true),
        ] {
            if !mounts.iter().any(|x| x.name == m.name) {
                mounts.push(m);
            }
        }
    }
    pod.metadata
        .annotations
        .get_or_insert_with(BTreeMap::new)
        .insert(ANNOTATION_ENVELOPE_INJECTED.to_owned(), "true".to_owned());
    pod
}

/// Both tokens from the pod's own annotations, readable by its `fsGroup`.
fn guard_auth_volume() -> Volume {
    let item = |path: &str, annotation: &str| DownwardAPIVolumeFile {
        path: path.to_owned(),
        field_ref: Some(ObjectFieldSelector {
            field_path: format!("metadata.annotations['{annotation}']"),
            api_version: None,
        }),
        mode: None,
        resource_field_ref: None,
        user: None,
    };
    Volume {
        name: VOL_GUARD_AUTH.to_owned(),
        downward_api: Some(DownwardAPIVolumeSource {
            default_mode: Some(0o440),
            items: Some(vec![
                item(GUARD_PROXY_TOKEN_FILE, ANNOTATION_GUARD_PROXY_TOKEN),
                item(GUARD_CONTROL_TOKEN_FILE, ANNOTATION_GUARD_CONTROL_TOKEN),
            ]),
            ..DownwardAPIVolumeSource::default()
        }),
        ..Volume::default()
    }
}

/// What the dispatcher knows about one dispatch's guard pod.
pub struct GuardPod<'a> {
    pub name: &'a str,
    pub job: &'a str,
    pub sidecar_image: &'a str,
    pub profile: &'a WorkerProfileSpec,
    /// The dispatch id, server URL, guard identity and both tokens.
    pub annotations: BTreeMap<String, String>,
    pub owner: Option<OwnerReference>,
    pub active_deadline_seconds: Option<i64>,
}

fn guard_pod_labels(job: &str) -> BTreeMap<String, String> {
    BTreeMap::from([
        (LABEL_ROLE.to_owned(), ROLE_GUARD_PROXY.to_owned()),
        (LABEL_GUARD_FOR.to_owned(), job.to_owned()),
    ])
}

/// The guard-proxy of a `pod`-placed profile, as its own restricted pod.
///
/// It is the sidecar container in forward mode behind the proxy token. It
/// carries neither the worker-profile label nor `job-name`, so neither the
/// webhook nor the worker's policies and status select it.
#[must_use]
pub fn guard_pod(g: &GuardPod<'_>) -> Pod {
    let cfg = EnvelopeConfig {
        sidecar_image: g.sidecar_image.to_owned(),
        worker_container: String::new(),
        guard_identity: g.annotations.get(ANNOTATION_GUARD_IDENTITY).cloned(),
        gpg: false,
        guard: g.profile.guard.clone(),
        posture: Posture::Nonroot,
        dispatched: g.annotations.contains_key(ANNOTATION_DISPATCH_ID),
        exec_interception: false,
        human_gate: g.profile.human_gate,
        dns_resolver: DEFAULT_DNS_RESOLVER.to_owned(),
        sidecar_resources: g.profile.sidecar_resources.clone(),
    };
    let container = guard_pod_container(&cfg);
    let mounted: Vec<String> =
        container.volume_mounts.iter().flatten().map(|m| m.name.clone()).collect();
    let mut volumes: Vec<Volume> =
        envelope_volumes(&cfg).into_iter().filter(|v| mounted.contains(&v.name)).collect();
    volumes.push(guard_auth_volume());

    let p = g.profile;
    Pod {
        metadata: ObjectMeta {
            name: Some(g.name.to_owned()),
            labels: Some(guard_pod_labels(g.job)),
            annotations: Some(g.annotations.clone()),
            owner_references: g.owner.clone().map(|o| vec![o]),
            ..ObjectMeta::default()
        },
        spec: Some(PodSpec {
            restart_policy: Some("Never".to_owned()),
            automount_service_account_token: Some(false),
            enable_service_links: Some(false),
            service_account_name: p.service_account_name.clone(),
            active_deadline_seconds: g.active_deadline_seconds,
            security_context: Some(PodSecurityContext {
                run_as_non_root: Some(true),
                fs_group: Some(PROXY_UID),
                seccomp_profile: Some(SeccompProfile {
                    type_: "RuntimeDefault".to_owned(),
                    localhost_profile: None,
                }),
                ..PodSecurityContext::default()
            }),
            containers: vec![container],
            volumes: Some(volumes),
            image_pull_secrets: p.image_pull_secrets.clone(),
            node_selector: p.node_selector.clone(),
            tolerations: p.tolerations.clone(),
            priority_class_name: p.priority_class_name().map(ToOwned::to_owned),
            ..PodSpec::default()
        }),
        status: None,
    }
}

fn guard_pod_container(cfg: &EnvelopeConfig) -> Container {
    let mut container = guard_proxy_container(cfg);
    container.restart_policy = None;
    container.command = Some(
        [
            "cctui-guard-proxy-entrypoint",
            "--mode=forward",
            "--listen=0.0.0.0:15001",
            "--health-listen=0.0.0.0:15002",
            "--server-gateway-listen=0.0.0.0:15003",
            "--policy=/var/run/guard-proxy/policy.json",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect(),
    );
    let env = container.env.get_or_insert_with(Vec::new);
    let proxy_token = format!("{GUARD_AUTH_DIR}/{GUARD_PROXY_TOKEN_FILE}");
    let control_token = format!("{GUARD_AUTH_DIR}/{GUARD_CONTROL_TOKEN_FILE}");
    upsert_env(env, "GUARD_PROXY_AUTH_TOKEN_FILE", &proxy_token);
    upsert_env(env, "GUARD_PROXY_CONTROL_TOKEN_FILE", &control_token);
    let mounts = container.volume_mounts.get_or_insert_with(Vec::new);
    mounts.push(mount(VOL_GUARD_AUTH, GUARD_AUTH_DIR, true));
    let ports = [
        ("proxy", GUARD_PORT_PROXY),
        ("control", GUARD_PORT_CONTROL),
        ("gateway", GUARD_PORT_GATEWAY),
    ];
    container.ports = Some(
        ports
            .into_iter()
            .map(|(name, port)| ContainerPort {
                name: Some(name.to_owned()),
                container_port: port,
                ..ContainerPort::default()
            })
            .collect(),
    );
    container
}

/// The Service a `pod`-placed worker dials its guard by, named like the pod.
#[must_use]
pub fn guard_service(name: &str, job: &str, owner: Option<OwnerReference>) -> Service {
    let port = |name: &str, port: i32| ServicePort {
        name: Some(name.to_owned()),
        port,
        target_port: Some(IntOrString::Int(port)),
        protocol: Some("TCP".to_owned()),
        ..ServicePort::default()
    };
    Service {
        metadata: ObjectMeta {
            name: Some(name.to_owned()),
            labels: Some(guard_pod_labels(job)),
            owner_references: owner.map(|o| vec![o]),
            ..ObjectMeta::default()
        },
        spec: Some(ServiceSpec {
            selector: Some(guard_pod_labels(job)),
            ports: Some(vec![
                port("proxy", GUARD_PORT_PROXY),
                port("control", GUARD_PORT_CONTROL),
                port("gateway", GUARD_PORT_GATEWAY),
            ]),
            ..ServiceSpec::default()
        }),
        status: None,
    }
}

fn net_init_container(cfg: &EnvelopeConfig) -> Container {
    Container {
        name: NET_INIT_CONTAINER.to_owned(),
        image: Some(cfg.sidecar_image.clone()),
        command: Some(vec!["cctui-worker-net-init".to_owned()]),
        security_context: Some(SecurityContext {
            run_as_user: Some(0),
            capabilities: Some(Capabilities {
                drop: Some(vec!["ALL".to_owned()]),
                add: Some(vec!["NET_ADMIN".to_owned()]),
            }),
            ..SecurityContext::default()
        }),
        env: Some(vec![EnvVar {
            name: NET_INIT_ENV_DNS_RESOLVER.to_owned(),
            value: Some(cfg.dns_resolver.clone()),
            value_from: None,
        }]),
        resources: Some(resources(
            &NET_INIT_RESOURCES,
            cfg.sidecar_resources.as_ref().and_then(|s| s.net_init.as_ref()),
        )),
        ..Container::default()
    }
}

fn guard_proxy_container(cfg: &EnvelopeConfig) -> Container {
    let mut env = Vec::new();
    if let Some(identity) = &cfg.guard_identity {
        env.push(EnvVar {
            name: "GUARD_PROXY_IDENTITY".to_owned(),
            value: Some(identity.clone()),
            value_from: None,
        });
    }
    let mut env_from = vec![EnvFromSource {
        config_map_ref: Some(ConfigMapEnvSource {
            name: CONFIGMAP_GUARD_PROXY_ENV.to_owned(),
            optional: Some(false),
        }),
        ..EnvFromSource::default()
    }];
    if let Some(guard) = &cfg.guard {
        env_from.extend(guard.env_from.iter().flatten().cloned());
        env.extend(guard.env.iter().flatten().cloned());
    }

    let mut mounts = vec![
        mount(VOL_PROXY_POLICY, "/var/run/guard-proxy", false),
        mount(VOL_GUARD_PROXY_INJECT, "/etc/guard-proxy", true),
        mount(VOL_GUARD_PROXY_CA, "/var/run/guard-proxy-ca", false),
        mount(VOL_GUARD_PROXY_SA_TOKEN, SA_TOKEN_DIR, true),
    ];
    if cfg.gpg {
        mounts.push(mount(VOL_GPG_AGENT, "/var/run/gpg-agent", false));
    }
    if let Some(identity) = cfg.guard.as_ref().and_then(|g| g.cloud_identity.as_ref()) {
        mounts.extend(identity.volume_mounts.iter().cloned());
        env.extend(identity.env.iter().flatten().cloned());
    }
    if cfg.dispatched {
        env.push(annotation_env("CCTUI_DISPATCH_ID", ANNOTATION_DISPATCH_ID));
        env.push(annotation_env("CCTUI_DISPATCH_SERVER_URL", ANNOTATION_SERVER_URL));
    }
    env.retain(|e| e.name != GUARD_ENV_HUMAN_GATE);
    if let Some(gate) = cfg.human_gate {
        env.push(EnvVar {
            name: GUARD_ENV_HUMAN_GATE.to_owned(),
            value: Some(gate.as_str().to_owned()),
            value_from: None,
        });
    }

    Container {
        name: GUARD_PROXY_CONTAINER.to_owned(),
        image: Some(cfg.sidecar_image.clone()),
        restart_policy: Some("Always".to_owned()),
        command: Some(
            [
                "cctui-guard-proxy-entrypoint",
                "--mode=transparent",
                "--listen=0.0.0.0:15001",
                "--health-listen=0.0.0.0:15002",
                "--policy=/var/run/guard-proxy/policy.json",
            ]
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
        ),
        env: (!env.is_empty()).then_some(env),
        env_from: Some(env_from),
        security_context: Some(guard_proxy_security_context(cfg.posture)),
        volume_mounts: Some(mounts),
        startup_probe: Some(Probe {
            http_get: Some(HTTPGetAction {
                path: Some("/health".to_owned()),
                port: IntOrString::Int(15002),
                ..HTTPGetAction::default()
            }),
            period_seconds: Some(2),
            failure_threshold: Some(30),
            ..Probe::default()
        }),
        resources: Some(resources(
            &GUARD_PROXY_RESOURCES,
            cfg.sidecar_resources.as_ref().and_then(|s| s.guard_proxy.as_ref()),
        )),
        ..Container::default()
    }
}

fn annotation_env(name: &str, annotation: &str) -> EnvVar {
    EnvVar {
        name: name.to_owned(),
        value: None,
        value_from: Some(EnvVarSource {
            field_ref: Some(ObjectFieldSelector {
                field_path: format!("metadata.annotations['{annotation}']"),
                api_version: None,
            }),
            ..EnvVarSource::default()
        }),
    }
}

fn envelope_volumes(cfg: &EnvelopeConfig) -> Vec<Volume> {
    let mut volumes = vec![
        empty_dir(VOL_HOME),
        empty_dir(VOL_OVERLAY),
        empty_dir(VOL_GUARD_STATE),
        empty_dir(VOL_PROXY_POLICY),
        Volume {
            name: VOL_GUARD_PROXY_INJECT.to_owned(),
            config_map: Some(ConfigMapVolumeSource {
                name: CONFIGMAP_GUARD_PROXY_INJECT.to_owned(),
                ..ConfigMapVolumeSource::default()
            }),
            ..Volume::default()
        },
        empty_dir(VOL_GUARD_PROXY_CA),
        sa_token_volume(),
    ];
    if cfg.gpg {
        volumes.push(empty_dir(VOL_GPG_AGENT));
    }
    volumes.retain(|v| has_overlay(cfg.posture) || v.name != VOL_OVERLAY);
    volumes
}

/// The projection the kubelet automounts (token, cluster CA, namespace), plus
/// the `cctui`-audience token the guard-proxy redeems for the dispatch.
fn sa_token_volume() -> Volume {
    let sources = vec![
        VolumeProjection {
            service_account_token: Some(ServiceAccountTokenProjection {
                path: "token".to_owned(),
                expiration_seconds: Some(SA_TOKEN_EXPIRATION_SECONDS),
                audience: None,
                user: None,
            }),
            ..VolumeProjection::default()
        },
        VolumeProjection {
            service_account_token: Some(ServiceAccountTokenProjection {
                path: CCTUI_TOKEN_FILE.to_owned(),
                expiration_seconds: Some(CCTUI_TOKEN_EXPIRATION_SECONDS),
                audience: Some(CCTUI_TOKEN_AUDIENCE.to_owned()),
                user: None,
            }),
            ..VolumeProjection::default()
        },
        VolumeProjection {
            config_map: Some(ConfigMapProjection {
                name: CONFIGMAP_KUBE_ROOT_CA.to_owned(),
                items: Some(vec![KeyToPath {
                    key: "ca.crt".to_owned(),
                    path: "ca.crt".to_owned(),
                    mode: None,
                    user: None,
                }]),
                optional: None,
            }),
            ..VolumeProjection::default()
        },
        VolumeProjection {
            downward_api: Some(DownwardAPIProjection {
                items: Some(vec![DownwardAPIVolumeFile {
                    path: "namespace".to_owned(),
                    field_ref: Some(ObjectFieldSelector {
                        field_path: "metadata.namespace".to_owned(),
                        api_version: None,
                    }),
                    mode: None,
                    resource_field_ref: None,
                    user: None,
                }]),
            }),
            ..VolumeProjection::default()
        },
    ];
    Volume {
        name: VOL_GUARD_PROXY_SA_TOKEN.to_owned(),
        projected: Some(ProjectedVolumeSource {
            sources: Some(sources),
            default_mode: None,
            default_user: None,
        }),
        ..Volume::default()
    }
}

fn empty_dir(name: &str) -> Volume {
    Volume {
        name: name.to_owned(),
        empty_dir: Some(EmptyDirVolumeSource::default()),
        ..Volume::default()
    }
}

fn mount(name: &str, path: &str, read_only: bool) -> VolumeMount {
    VolumeMount {
        name: name.to_owned(),
        mount_path: path.to_owned(),
        read_only: read_only.then_some(true),
        ..VolumeMount::default()
    }
}

fn upsert_env(env: &mut Vec<EnvVar>, name: &str, value: &str) {
    if let Some(existing) = env.iter_mut().find(|e| e.name == name) {
        existing.value = Some(value.to_owned());
        existing.value_from = None;
    } else {
        env.push(EnvVar { name: name.to_owned(), value: Some(value.to_owned()), value_from: None });
    }
}

fn resources(
    defaults: &[(&str, &str, &str)],
    overrides: Option<&ResourceRequirements>,
) -> ResourceRequirements {
    let requests = defaults.iter().map(|(name, request, _)| (*name, *request));
    let limits = defaults.iter().map(|(name, _, limit)| (*name, *limit));
    ResourceRequirements {
        requests: Some(merged(requests, overrides.and_then(|o| o.requests.as_ref()))),
        limits: Some(merged(limits, overrides.and_then(|o| o.limits.as_ref()))),
        claims: overrides.and_then(|o| o.claims.clone()),
    }
}

fn merged<'a>(
    defaults: impl Iterator<Item = (&'a str, &'a str)>,
    overrides: Option<&BTreeMap<String, Quantity>>,
) -> BTreeMap<String, Quantity> {
    let mut map: BTreeMap<String, Quantity> =
        defaults.map(|(name, value)| (name.to_owned(), Quantity(value.to_owned()))).collect();
    map.extend(overrides.into_iter().flatten().map(|(k, v)| (k.clone(), v.clone())));
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMG: &str = "registry.example.com/cctui-worker:test";

    fn lean_pod() -> Pod {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "worker-lean",
                "labels": { LABEL_WORKER_PROFILE: "lean" },
                "annotations": {}
            },
            "spec": {
                "containers": [
                    { "name": "worker", "image": "registry.example.com/worker:latest" }
                ]
            }
        }))
        .unwrap()
    }

    fn full_stack_pod() -> Pod {
        serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "worker-full",
                "labels": { LABEL_WORKER_PROFILE: "full" },
                "annotations": {}
            },
            "spec": {
                "initContainers": [
                    { "name": "migrate", "image": "registry.example.com/db-migrate:latest" }
                ],
                "containers": [
                    { "name": "worker", "image": "registry.example.com/worker:latest" },
                    {
                        "name": "db",
                        "image": "registry.example.com/postgres:16",
                        "securityContext": { "runAsUser": 999 }
                    }
                ]
            }
        }))
        .unwrap()
    }

    fn apply(pod: &Pod) -> Pod {
        apply_with(pod, None)
    }

    fn apply_with(pod: &Pod, guard: Option<&GuardSpec>) -> Pod {
        apply_flags(pod, guard, ProfileFlags::default())
    }

    fn apply_flags(pod: &Pod, guard: Option<&GuardSpec>, flags: ProfileFlags) -> Pod {
        let patch = match mutate_pod(pod, IMG, guard, flags).expect("mutate_pod must not error") {
            MutationOutcome::Patch(p) => p,
            other => panic!("eligible pod must yield a patch, got {other:?}"),
        };
        let mut doc = serde_json::to_value(pod).unwrap();
        json_patch::patch(&mut doc, &patch.0).expect("patch applies");
        serde_json::from_value(doc).unwrap()
    }

    fn worker<'a>(pod: &'a Pod, name: &str) -> &'a Container {
        pod.spec.as_ref().unwrap().containers.iter().find(|c| c.name == name).unwrap()
    }

    #[test]
    fn lean_pod_gets_full_envelope() {
        let out = apply(&lean_pod());
        let spec = out.spec.as_ref().unwrap();
        let init = spec.init_containers.as_ref().unwrap();
        assert_eq!(init[0].name, "net-init");
        assert_eq!(init[1].name, "guard-proxy");
        assert_eq!(init[1].restart_policy.as_deref(), Some("Always"));
        assert_eq!(init[0].command.as_ref().unwrap(), &["cctui-worker-net-init"]);

        assert_eq!(spec.security_context.as_ref().unwrap().fs_group, Some(1000));

        let vols: Vec<&str> =
            spec.volumes.as_ref().unwrap().iter().map(|v| v.name.as_str()).collect();
        for want in [
            "home",
            "overlay",
            "guard-state",
            "proxy-policy",
            "guard-proxy-inject",
            "guard-proxy-ca",
            "guard-proxy-sa-token",
        ] {
            assert!(vols.contains(&want), "missing volume {want}");
        }
        assert!(!vols.contains(&"gpg-agent"));

        let w = worker(&out, "worker");
        let sc = w.security_context.as_ref().unwrap();
        assert_eq!(sc.run_as_user, Some(0));
        assert_eq!(sc.app_armor_profile.as_ref().unwrap().type_, "Unconfined");
        let add = sc.capabilities.as_ref().unwrap().add.as_ref().unwrap();
        assert!(add.contains(&"SYS_ADMIN".to_owned()));
        assert!(add.contains(&"SETPCAP".to_owned()));
        assert_eq!(sc.capabilities.as_ref().unwrap().drop.as_ref().unwrap(), &["ALL"]);

        let net_mode =
            w.env.as_ref().unwrap().iter().find(|e| e.name == "WORKER_NET_MODE").unwrap();
        assert_eq!(net_mode.value.as_deref(), Some("transparent-external"));

        let ca = w.env.as_ref().unwrap().iter().find(|e| e.name == "NODE_EXTRA_CA_CERTS").unwrap();
        assert_eq!(ca.value.as_deref(), Some("/var/run/guard-proxy-ca/ca.pem"));

        let gp = &init[1];
        assert_eq!(
            gp.env_from.as_ref().unwrap()[0].config_map_ref.as_ref().unwrap().name,
            "guard-proxy-env"
        );
        assert!(gp.env.is_none(), "no identity annotation -> ConfigMap default only");
    }

    fn golden_mount(name: &str, path: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "mountPath": path })
    }

    fn golden_ro_mount(name: &str, path: &str) -> serde_json::Value {
        serde_json::json!({ "name": name, "mountPath": path, "readOnly": true })
    }

    fn golden_net_init_resources() -> serde_json::Value {
        serde_json::json!({
            "requests": { "cpu": "10m", "memory": "16Mi", "ephemeral-storage": "4Mi" },
            "limits": { "cpu": "100m", "memory": "64Mi", "ephemeral-storage": "16Mi" }
        })
    }

    fn golden_guard_proxy_resources() -> serde_json::Value {
        serde_json::json!({
            "requests": { "cpu": "25m", "memory": "32Mi", "ephemeral-storage": "32Mi" },
            "limits": { "cpu": "500m", "memory": "256Mi", "ephemeral-storage": "128Mi" }
        })
    }

    fn golden_net_init() -> serde_json::Value {
        serde_json::json!({
            "name": "net-init",
            "image": IMG,
            "command": ["cctui-worker-net-init"],
            "securityContext": {
                "runAsUser": 0,
                "capabilities": { "drop": ["ALL"], "add": ["NET_ADMIN"] }
            },
            "env": [{ "name": "WORKER_DNS_RESOLVER", "value": "cctui-worker-dns" }],
            "resources": golden_net_init_resources()
        })
    }

    fn golden_root_guard_proxy_sc() -> serde_json::Value {
        serde_json::json!({
            "runAsUser": 1337,
            "runAsGroup": 1337,
            "allowPrivilegeEscalation": false,
            "capabilities": { "drop": ["ALL"] }
        })
    }

    fn golden_restricted(uid: i64) -> serde_json::Value {
        serde_json::json!({
            "runAsUser": uid,
            "runAsGroup": uid,
            "runAsNonRoot": true,
            "allowPrivilegeEscalation": false,
            "capabilities": { "drop": ["ALL"] },
            "seccompProfile": { "type": "RuntimeDefault" }
        })
    }

    fn golden_guard_proxy(security_context: &serde_json::Value) -> serde_json::Value {
        let sa_token_dir = "/var/run/secrets/kubernetes.io/serviceaccount";
        serde_json::json!({
            "name": "guard-proxy",
            "image": IMG,
            "restartPolicy": "Always",
            "command": [
                "cctui-guard-proxy-entrypoint",
                "--mode=transparent",
                "--listen=0.0.0.0:15001",
                "--health-listen=0.0.0.0:15002",
                "--policy=/var/run/guard-proxy/policy.json"
            ],
            "envFrom": [{ "configMapRef": { "name": "guard-proxy-env", "optional": false } }],
            "securityContext": security_context,
            "volumeMounts": [
                golden_mount("proxy-policy", "/var/run/guard-proxy"),
                golden_ro_mount("guard-proxy-inject", "/etc/guard-proxy"),
                golden_mount("guard-proxy-ca", "/var/run/guard-proxy-ca"),
                golden_ro_mount("guard-proxy-sa-token", sa_token_dir)
            ],
            "startupProbe": {
                "httpGet": { "path": "/health", "port": 15002 },
                "periodSeconds": 2,
                "failureThreshold": 30
            },
            "resources": golden_guard_proxy_resources()
        })
    }

    fn golden_worker() -> serde_json::Value {
        serde_json::json!({
            "name": "worker",
            "image": "registry.example.com/worker:latest",
            "securityContext": {
                "runAsUser": 0,
                "appArmorProfile": { "type": "Unconfined" },
                "capabilities": { "drop": ["ALL"], "add": WORKER_ADDED_CAPS }
            },
            "env": [
                { "name": "WORKER_NET_MODE", "value": "transparent-external" },
                { "name": "NODE_EXTRA_CA_CERTS", "value": "/var/run/guard-proxy-ca/ca.pem" }
            ],
            "volumeMounts": [
                golden_mount("home", "/home/worker"),
                golden_mount("overlay", "/overlay"),
                golden_mount("guard-state", "/var/run/workflow-guard"),
                golden_mount("proxy-policy", "/var/run/guard-proxy"),
                golden_mount("guard-proxy-ca", "/var/run/guard-proxy-ca")
            ]
        })
    }

    fn golden_nonroot_worker() -> serde_json::Value {
        serde_json::json!({
            "name": "worker",
            "image": "registry.example.com/worker:latest",
            "securityContext": golden_restricted(1000),
            "env": [
                { "name": "WORKER_NET_MODE", "value": "transparent-external" },
                { "name": "NODE_EXTRA_CA_CERTS", "value": "/var/run/guard-proxy-ca/ca.pem" }
            ],
            "volumeMounts": [
                golden_mount("home", "/home/worker"),
                golden_mount("guard-state", "/var/run/workflow-guard"),
                golden_mount("proxy-policy", "/var/run/guard-proxy"),
                golden_mount("guard-proxy-ca", "/var/run/guard-proxy-ca")
            ]
        })
    }

    fn golden_volumes() -> serde_json::Value {
        let empty = |name: &str| serde_json::json!({ "name": name, "emptyDir": {} });
        serde_json::json!([
            empty("home"),
            empty("overlay"),
            empty("guard-state"),
            empty("proxy-policy"),
            { "name": "guard-proxy-inject", "configMap": { "name": "guard-proxy-inject" } },
            empty("guard-proxy-ca"),
            {
                "name": "guard-proxy-sa-token",
                "projected": {
                    "sources": [
                        { "serviceAccountToken": { "path": "token", "expirationSeconds": 3600 } },
                        {
                            "serviceAccountToken": {
                                "path": "cctui-token",
                                "expirationSeconds": 600,
                                "audience": "cctui"
                            }
                        },
                        {
                            "configMap": {
                                "name": "kube-root-ca.crt",
                                "items": [{ "key": "ca.crt", "path": "ca.crt" }]
                            }
                        },
                        {
                            "downwardAPI": {
                                "items": [{
                                    "path": "namespace",
                                    "fieldRef": { "fieldPath": "metadata.namespace" }
                                }]
                            }
                        }
                    ]
                }
            }
        ])
    }

    /// `lean_pod` as injected before profiles could carry `spec.guard`.
    fn lean_pod_injected_golden() -> serde_json::Value {
        serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "worker-lean",
                "labels": { LABEL_WORKER_PROFILE: "lean" },
                "annotations": { ANNOTATION_ENVELOPE_INJECTED: "true" }
            },
            "spec": {
                "initContainers": [
                    golden_net_init(),
                    golden_guard_proxy(&golden_root_guard_proxy_sc())
                ],
                "containers": [golden_worker()],
                "securityContext": { "fsGroup": 1000 },
                "volumes": golden_volumes()
            }
        })
    }

    fn with_annotation(mut pod: Pod, key: &str, value: &str) -> Pod {
        pod.metadata
            .annotations
            .get_or_insert_with(BTreeMap::new)
            .insert(key.to_owned(), value.to_owned());
        pod
    }

    fn nonroot_pod() -> Pod {
        with_annotation(lean_pod(), ANNOTATION_POSTURE, "nonroot")
    }

    /// `lean_pod` under `posture: nonroot`: the pod the restricted-worker canary ran.
    fn nonroot_pod_injected_golden() -> serde_json::Value {
        let mut volumes = golden_volumes();
        volumes.as_array_mut().unwrap().retain(|v| v["name"] != "overlay");
        serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "worker-lean",
                "labels": { LABEL_WORKER_PROFILE: "lean" },
                "annotations": {
                    ANNOTATION_POSTURE: "nonroot",
                    ANNOTATION_ENVELOPE_INJECTED: "true"
                }
            },
            "spec": {
                "initContainers": [
                    golden_net_init(),
                    golden_guard_proxy(&golden_restricted(1337))
                ],
                "containers": [golden_nonroot_worker()],
                "securityContext": { "fsGroup": 1000 },
                "volumes": volumes
            }
        })
    }

    #[test]
    fn nonroot_posture_renders_the_restricted_golden_pod() {
        let out = serde_json::to_value(apply(&nonroot_pod())).unwrap();
        assert_eq!(out, nonroot_pod_injected_golden());
    }

    #[test]
    fn root_posture_annotation_renders_like_no_annotation() {
        let pod = with_annotation(lean_pod(), ANNOTATION_POSTURE, "root");
        let mut out = serde_json::to_value(apply(&pod)).unwrap();
        out["metadata"]["annotations"].as_object_mut().unwrap().remove(ANNOTATION_POSTURE);
        assert_eq!(out, lean_pod_injected_golden());
    }

    #[test]
    fn nonroot_posture_drops_warm_repo_dir_and_keeps_other_env() {
        let env = vec![
            EnvVar {
                name: "LOG_LEVEL".to_owned(),
                value: Some("info".to_owned()),
                value_from: None,
            },
            EnvVar {
                name: WARM_REPO_DIR_ENV.to_owned(),
                value: Some("/warm".to_owned()),
                value_from: None,
            },
        ];
        let mut pod = nonroot_pod();
        pod.spec.as_mut().unwrap().containers[0].env = Some(env.clone());
        let out = apply(&pod);
        let names: Vec<&str> =
            worker(&out, "worker").env.as_ref().unwrap().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["LOG_LEVEL", "WORKER_NET_MODE", "NODE_EXTRA_CA_CERTS"]);

        let mut root = lean_pod();
        root.spec.as_mut().unwrap().containers[0].env = Some(env);
        let out = apply(&root);
        let root_env = worker(&out, "worker").env.as_ref().unwrap();
        assert!(
            root_env.iter().any(|e| e.name == WARM_REPO_DIR_ENV),
            "root keeps the overlay warm cache"
        );
    }

    #[test]
    fn nonroot_posture_with_gpg_keeps_the_gpg_agent_and_no_overlay() {
        let pod = with_annotation(nonroot_pod(), ANNOTATION_GPG_SIGNING, "true");
        let out = apply(&pod);
        let spec = out.spec.as_ref().unwrap();
        let vols: Vec<&str> =
            spec.volumes.as_ref().unwrap().iter().map(|v| v.name.as_str()).collect();
        assert!(vols.contains(&"gpg-agent"));
        assert!(!vols.contains(&"overlay"));
        let mounts: Vec<&str> = worker(&out, "worker")
            .volume_mounts
            .as_ref()
            .unwrap()
            .iter()
            .map(|m| m.name.as_str())
            .collect();
        assert_eq!(mounts, ["home", "guard-state", "proxy-policy", "guard-proxy-ca", "gpg-agent"]);
    }

    #[test]
    fn nonroot_posture_keeps_a_preset_fs_group_and_passthrough_containers() {
        let mut pod = with_annotation(full_stack_pod(), ANNOTATION_POSTURE, "nonroot");
        pod.spec.as_mut().unwrap().security_context =
            Some(PodSecurityContext { fs_group: Some(2000), ..PodSecurityContext::default() });
        let out = apply(&pod);
        assert_eq!(
            out.spec.as_ref().unwrap().security_context.as_ref().unwrap().fs_group,
            Some(2000)
        );
        assert_eq!(
            serde_json::to_value(worker(&out, "db")).unwrap(),
            serde_json::to_value(worker(&pod, "db")).unwrap()
        );
        assert_eq!(
            worker(&out, "worker").security_context,
            Some(worker_security_context(Posture::Nonroot))
        );
        let net_init = &out.spec.as_ref().unwrap().init_containers.as_ref().unwrap()[0];
        assert_eq!(net_init.name, NET_INIT_CONTAINER);
        let caps = net_init.security_context.as_ref().unwrap().capabilities.as_ref().unwrap();
        assert_eq!(caps.add.as_deref(), Some(&["NET_ADMIN".to_owned()][..]));
    }

    fn worker_caps(pod: &Pod, exec_interception: bool) -> Vec<String> {
        let flags = ProfileFlags { exec_interception, ..ProfileFlags::default() };
        let patch = match mutate_pod(pod, IMG, None, flags).unwrap() {
            MutationOutcome::Patch(p) => p,
            other => panic!("eligible pod must yield a patch, got {other:?}"),
        };
        let mut doc = serde_json::to_value(pod).unwrap();
        json_patch::patch(&mut doc, &patch.0).unwrap();
        let out: Pod = serde_json::from_value(doc).unwrap();
        let sc = worker(&out, "worker").security_context.clone().unwrap();
        sc.capabilities.and_then(|c| c.add).unwrap_or_default()
    }

    #[test]
    fn only_an_exec_interception_root_worker_gains_sys_ptrace() {
        let ptrace = EXEC_INTERCEPTION_CAP.to_owned();
        let plain = worker_caps(&lean_pod(), false);
        assert!(!plain.contains(&ptrace), "{plain:?}");
        let opted = worker_caps(&lean_pod(), true);
        assert!(opted.contains(&ptrace), "{opted:?}");
        assert!(WORKER_ADDED_CAPS.iter().all(|c| opted.contains(&(*c).to_owned())), "{opted:?}");
        let nonroot = worker_caps(&nonroot_pod(), true);
        assert!(nonroot.is_empty(), "nonroot never gains a capability: {nonroot:?}");
    }

    #[test]
    fn unknown_posture_annotation_fails_closed() {
        let pod = with_annotation(lean_pod(), ANNOTATION_POSTURE, "restricted");
        let err = mutate_pod(&pod, IMG, None, ProfileFlags::default()).unwrap_err().to_string();
        assert!(err.contains("restricted") && err.contains(ANNOTATION_POSTURE), "{err}");
    }

    #[test]
    fn envelope_mounts_and_caps_per_posture() {
        assert_eq!(worker_envelope_mounts(Posture::Root), WORKER_ENVELOPE_MOUNTS);
        assert_eq!(
            worker_envelope_mounts(Posture::Nonroot),
            [VOL_HOME, VOL_GUARD_STATE, VOL_PROXY_POLICY, VOL_GUARD_PROXY_CA]
        );
        assert_eq!(worker_added_caps(Posture::Root), WORKER_ADDED_CAPS);
        assert!(worker_added_caps(Posture::Nonroot).is_empty());
    }

    #[test]
    fn profile_without_guard_renders_the_golden_pod() {
        let golden = lean_pod_injected_golden();
        let out = serde_json::to_value(apply(&lean_pod())).unwrap();
        assert_eq!(out, golden);
        let empty_guard = GuardSpec::default();
        let out = serde_json::to_value(apply_with(&lean_pod(), Some(&empty_guard))).unwrap();
        assert_eq!(out, golden, "an empty spec.guard renders like no spec.guard");
    }

    fn pod_guard() -> GuardSpec {
        GuardSpec { placement: Some(GuardPlacement::Pod), ..GuardSpec::default() }
    }

    #[test]
    fn an_explicit_sidecar_placement_renders_the_golden_pod() {
        let sidecar =
            GuardSpec { placement: Some(GuardPlacement::Sidecar), ..GuardSpec::default() };
        let out = serde_json::to_value(apply_with(&lean_pod(), Some(&sidecar))).unwrap();
        assert_eq!(out, lean_pod_injected_golden());
        let out = serde_json::to_value(apply_with(&nonroot_pod(), Some(&sidecar))).unwrap();
        assert_eq!(out, nonroot_pod_injected_golden());
    }

    fn caps_added(c: &Container) -> Vec<String> {
        c.security_context
            .as_ref()
            .and_then(|sc| sc.capabilities.as_ref())
            .and_then(|caps| caps.add.clone())
            .unwrap_or_default()
    }

    #[test]
    fn pod_placement_renders_a_restricted_worker_with_no_net_init_and_no_sidecar() {
        let pod = with_annotation(nonroot_pod(), ANNOTATION_DISPATCH_ID, "d-1");
        let out = apply_with(&pod, Some(&pod_guard()));
        let spec = out.spec.as_ref().unwrap();
        assert!(spec.init_containers.as_ref().is_none_or(Vec::is_empty), "no net-init/sidecar");
        assert_eq!(spec.security_context.as_ref().unwrap().fs_group, Some(1000));
        let vols: Vec<&str> =
            spec.volumes.as_ref().unwrap().iter().map(|v| v.name.as_str()).collect();
        assert_eq!(vols, EXTERNAL_GUARD_WORKER_VOLUMES);

        let w = worker(&out, "worker");
        assert_eq!(
            serde_json::to_value(w.security_context.as_ref().unwrap()).unwrap(),
            golden_restricted(1000)
        );
        assert!(caps_added(w).is_empty(), "no NET_ADMIN, no capability at all");
        let env = |name: &str| w.env.iter().flatten().find(|e| e.name == name).cloned();
        assert_eq!(env("WORKER_NET_MODE").unwrap().value.as_deref(), Some("external-pod"));
        let host = env("GUARD_PROXY_HOST").unwrap().value_from.unwrap().field_ref.unwrap();
        assert_eq!(host.field_path, "metadata.annotations['cctui.dev/guard-service']");
        assert!(env("CCTUI_DISPATCH_ID").is_none());

        let mounts: Vec<(&str, &str, Option<bool>)> = w
            .volume_mounts
            .iter()
            .flatten()
            .map(|m| (m.name.as_str(), m.mount_path.as_str(), m.read_only))
            .collect();
        assert!(mounts.contains(&("guard-auth", "/var/run/guard-auth", Some(true))), "{mounts:?}");
        assert!(!mounts.iter().any(|m| m.0 == VOL_GUARD_PROXY_SA_TOKEN || m.0 == VOL_OVERLAY));
        let auth = spec.volumes.as_ref().unwrap().iter().find(|v| v.name == "guard-auth").unwrap();
        let items = auth.downward_api.as_ref().unwrap().items.as_ref().unwrap();
        let paths: Vec<(&str, &str)> = items
            .iter()
            .map(|i| (i.path.as_str(), i.field_ref.as_ref().unwrap().field_path.as_str()))
            .collect();
        assert_eq!(
            paths,
            [
                ("proxy-token", "metadata.annotations['cctui.dev/guard-proxy-token']"),
                ("control-token", "metadata.annotations['cctui.dev/guard-control-token']"),
            ]
        );
        let again = mutate_pod(&out, IMG, Some(&pod_guard()), ProfileFlags::default()).unwrap();
        assert!(matches!(again, MutationOutcome::AlreadyInjected));
    }

    fn guard_pod_profile() -> WorkerProfileSpec {
        serde_json::from_value(serde_json::json!({
            "image": "registry.example.com/worker:latest",
            "posture": "nonroot",
            "serviceAccountName": "cctui-worker",
            "humanGate": "draftPrOnly",
            "tolerations": [{ "key": "dedicated", "operator": "Equal", "value": "cctui-worker" }],
            "guard": { "placement": "pod" },
        }))
        .unwrap()
    }

    #[test]
    fn the_guard_pod_is_a_restricted_forward_proxy_behind_both_tokens() {
        let profile = guard_pod_profile();
        let annotations = BTreeMap::from([
            (ANNOTATION_DISPATCH_ID.to_owned(), "d-1".to_owned()),
            (ANNOTATION_SERVER_URL.to_owned(), "http://cctui.test:8700".to_owned()),
            (ANNOTATION_GUARD_IDENTITY.to_owned(), "acme".to_owned()),
            (ANNOTATION_GUARD_PROXY_TOKEN.to_owned(), "p".repeat(43)),
            (ANNOTATION_GUARD_CONTROL_TOKEN.to_owned(), "c".repeat(43)),
        ]);
        let owner = OwnerReference {
            api_version: "batch/v1".to_owned(),
            kind: "Job".to_owned(),
            name: "cctui-worker-abc".to_owned(),
            uid: "u-1".to_owned(),
            ..OwnerReference::default()
        };
        let pod = guard_pod(&GuardPod {
            name: "cctui-worker-abc-guard",
            job: "cctui-worker-abc",
            sidecar_image: IMG,
            profile: &profile,
            annotations,
            owner: Some(owner),
            active_deadline_seconds: Some(3900),
        });
        let labels = pod.metadata.labels.as_ref().unwrap();
        assert_eq!(labels.get(LABEL_ROLE).map(String::as_str), Some("guard-proxy"));
        assert_eq!(labels.get(LABEL_GUARD_FOR).map(String::as_str), Some("cctui-worker-abc"));
        assert!(!labels.contains_key(LABEL_WORKER_PROFILE) && !labels.contains_key("job-name"));
        assert_eq!(pod.metadata.owner_references.as_ref().unwrap()[0].uid, "u-1");

        let spec = pod.spec.as_ref().unwrap();
        assert_eq!(spec.restart_policy.as_deref(), Some("Never"));
        assert_eq!(spec.automount_service_account_token, Some(false));
        assert_eq!(spec.service_account_name.as_deref(), Some("cctui-worker"));
        assert_eq!(spec.active_deadline_seconds, Some(3900));
        assert_eq!(spec.tolerations.as_ref().unwrap().len(), 1);
        assert!(spec.init_containers.is_none());
        let [gp] = spec.containers.as_slice() else { panic!("one container") };
        assert_eq!(gp.name, GUARD_PROXY_CONTAINER);
        assert!(gp.restart_policy.is_none());
        assert_eq!(
            serde_json::to_value(gp.security_context.as_ref().unwrap()).unwrap(),
            golden_restricted(PROXY_UID)
        );
        assert!(caps_added(gp).is_empty());
        let command = gp.command.as_ref().unwrap();
        assert!(command.contains(&"--mode=forward".to_owned()), "{command:?}");
        assert!(command.contains(&"--server-gateway-listen=0.0.0.0:15003".to_owned()));
        let env = |name: &str| gp.env.iter().flatten().find(|e| e.name == name).cloned();
        assert_eq!(
            env("GUARD_PROXY_AUTH_TOKEN_FILE").unwrap().value.as_deref(),
            Some("/var/run/guard-auth/proxy-token")
        );
        assert_eq!(
            env("GUARD_PROXY_CONTROL_TOKEN_FILE").unwrap().value.as_deref(),
            Some("/var/run/guard-auth/control-token")
        );
        assert_eq!(env("GUARD_PROXY_IDENTITY").unwrap().value.as_deref(), Some("acme"));
        assert_eq!(env(GUARD_ENV_HUMAN_GATE).unwrap().value.as_deref(), Some("draftPrOnly"));
        assert!(env("CCTUI_DISPATCH_ID").unwrap().value_from.is_some());
        let rendered = serde_json::to_string(gp).unwrap();
        assert!(!rendered.contains(&"p".repeat(43)), "tokens arrive by file, never by env");

        let vols: Vec<&str> =
            spec.volumes.as_ref().unwrap().iter().map(|v| v.name.as_str()).collect();
        for want in [
            "guard-auth",
            "guard-proxy-ca",
            "guard-proxy-inject",
            "guard-proxy-sa-token",
            "proxy-policy",
        ] {
            assert!(vols.contains(&want), "missing {want} in {vols:?}");
        }
        for worker_only in [VOL_HOME, VOL_OVERLAY, VOL_GUARD_STATE, VOL_GPG_AGENT] {
            assert!(!vols.contains(&worker_only), "{worker_only} in {vols:?}");
        }
    }

    #[test]
    fn the_guard_service_selects_only_its_guard_pod() {
        let svc = guard_service("cctui-worker-abc-guard", "cctui-worker-abc", None);
        let spec = svc.spec.unwrap();
        let selector = spec.selector.unwrap();
        assert_eq!(selector.get(LABEL_GUARD_FOR).map(String::as_str), Some("cctui-worker-abc"));
        assert_eq!(selector.get(LABEL_ROLE).map(String::as_str), Some("guard-proxy"));
        let ports: Vec<i32> = spec.ports.unwrap().iter().map(|p| p.port).collect();
        assert_eq!(ports, [15001, 15002, 15003]);
    }

    #[test]
    fn cloud_identity_reaches_only_the_guard_proxy() {
        let guard: GuardSpec = serde_json::from_value(serde_json::json!({
            "cloudIdentity": {
                "volumeMounts": [{
                    "name": "eks-pod-identity-token",
                    "mountPath": "/var/run/secrets/pods.eks.amazonaws.com/serviceaccount",
                    "readOnly": true
                }],
                "env": [{
                    "name": "AWS_CONTAINER_CREDENTIALS_FULL_URI",
                    "value": "http://169.254.170.23/v1/credentials"
                }]
            }
        }))
        .unwrap();
        let out = apply_with(&lean_pod(), Some(&guard));
        let spec = out.spec.as_ref().unwrap();
        let mounts_identity = |c: &Container| {
            c.volume_mounts.iter().flatten().any(|m| m.name == "eks-pod-identity-token")
        };
        let inits = spec.init_containers.as_ref().unwrap();
        let gp = inits.iter().find(|c| c.name == GUARD_PROXY_CONTAINER).unwrap();
        assert!(mounts_identity(gp));
        assert!(gp.env.iter().flatten().any(|e| e.name == "AWS_CONTAINER_CREDENTIALS_FULL_URI"));
        assert!(!inits.iter().filter(|c| c.name != GUARD_PROXY_CONTAINER).any(mounts_identity));
        assert!(!spec.containers.iter().any(mounts_identity));
        assert!(
            !spec.volumes.iter().flatten().any(|v| v.name == "eks-pod-identity-token"),
            "the identity webhook owns the volume"
        );
    }

    #[test]
    fn guard_env_and_env_from_render_after_the_global_config_map() {
        let mut pod = lean_pod();
        pod.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(ANNOTATION_GUARD_IDENTITY.to_owned(), "release-bot".to_owned());
        let guard: GuardSpec = serde_json::from_value(serde_json::json!({
            "envFrom": [
                { "configMapRef": { "name": "guard-proxy-env-canary" } },
                { "secretRef": { "name": "guard-proxy-canary-secret" } }
            ],
            "env": [{ "name": "GUARD_PROXY_ALLOW_PRIVATE_IPS", "value": "0" }]
        }))
        .unwrap();
        let out = apply_with(&pod, Some(&guard));
        let gp = &out.spec.as_ref().unwrap().init_containers.as_ref().unwrap()[1];

        let sources: Vec<String> = gp
            .env_from
            .as_ref()
            .unwrap()
            .iter()
            .map(|s| {
                s.config_map_ref
                    .as_ref()
                    .map(|c| format!("cm:{}", c.name))
                    .or_else(|| s.secret_ref.as_ref().map(|r| format!("secret:{}", r.name)))
                    .unwrap()
            })
            .collect();
        assert_eq!(
            sources,
            ["cm:guard-proxy-env", "cm:guard-proxy-env-canary", "secret:guard-proxy-canary-secret"]
        );

        let env: Vec<(&str, Option<&str>)> = gp
            .env
            .as_ref()
            .unwrap()
            .iter()
            .map(|e| (e.name.as_str(), e.value.as_deref()))
            .collect();
        assert_eq!(
            env,
            [
                ("GUARD_PROXY_IDENTITY", Some("release-bot")),
                ("GUARD_PROXY_ALLOW_PRIVATE_IPS", Some("0")),
            ]
        );

        let w = worker(&out, "worker");
        assert!(
            !w.env.as_ref().unwrap().iter().any(|e| e.name.starts_with("GUARD_PROXY_")),
            "guard settings never reach the worker"
        );
        assert!(w.env_from.is_none());
    }

    #[test]
    fn only_the_guard_proxy_mounts_the_service_account_token() {
        let out = apply(&lean_pod());
        let spec = out.spec.as_ref().unwrap();

        let vol = spec.volumes.as_ref().unwrap().iter().find(|v| v.name == "guard-proxy-sa-token");
        let sources = vol.unwrap().projected.as_ref().unwrap().sources.as_ref().unwrap();
        let tokens: Vec<_> =
            sources.iter().filter_map(|s| s.service_account_token.as_ref()).collect();
        assert_eq!(tokens.len(), 2);
        assert_eq!(tokens[0].path, "token");
        assert_eq!(tokens[0].expiration_seconds, Some(3600));
        assert!(tokens[0].audience.is_none(), "default audience: same token the kubelet mounts");
        assert_eq!(tokens[1].path, CCTUI_TOKEN_FILE);
        assert_eq!(tokens[1].audience.as_deref(), Some("cctui"));
        assert!(tokens[1].expiration_seconds.is_some_and(|s| s <= 600));
        let ca = sources.iter().find_map(|s| s.config_map.as_ref()).unwrap();
        assert_eq!(ca.name, "kube-root-ca.crt");
        let ns = sources.iter().find_map(|s| s.downward_api.as_ref()).unwrap();
        assert_eq!(
            ns.items.as_ref().unwrap()[0].field_ref.as_ref().unwrap().field_path,
            "metadata.namespace"
        );

        let gp = &spec.init_containers.as_ref().unwrap()[1];
        let m = gp
            .volume_mounts
            .as_ref()
            .unwrap()
            .iter()
            .find(|m| m.name == "guard-proxy-sa-token")
            .expect("guard-proxy mounts the token");
        assert_eq!(m.mount_path, "/var/run/secrets/kubernetes.io/serviceaccount");
        assert_eq!(m.read_only, Some(true));

        let w = worker(&out, "worker");
        assert!(
            !w.volume_mounts.as_ref().unwrap().iter().any(|m| m.name == "guard-proxy-sa-token"),
            "the agent container must not see the service-account token"
        );
        let ni = &spec.init_containers.as_ref().unwrap()[0];
        assert!(ni.volume_mounts.is_none());
    }

    #[test]
    fn a_dispatched_pod_hands_its_dispatch_to_the_guard_proxy_only() {
        let pod = with_annotation(lean_pod(), ANNOTATION_DISPATCH_ID, "d-1");
        let pod = with_annotation(pod, ANNOTATION_SERVER_URL, "http://cctui.test:8700");
        let out = apply(&pod);
        let spec = out.spec.as_ref().unwrap();
        let gp = &spec.init_containers.as_ref().unwrap()[1];
        let field = |name: &str| {
            gp.env
                .iter()
                .flatten()
                .find(|e| e.name == name)
                .and_then(|e| e.value_from.as_ref()?.field_ref.as_ref())
                .map(|f| f.field_path.clone())
        };
        assert_eq!(
            field("CCTUI_DISPATCH_ID").as_deref(),
            Some("metadata.annotations['cctui.dev/dispatch-id']")
        );
        assert_eq!(
            field("CCTUI_DISPATCH_SERVER_URL").as_deref(),
            Some("metadata.annotations['cctui.dev/server-url']")
        );
        let w = worker(&out, "worker");
        assert!(!w.env.iter().flatten().any(|e| e.name.starts_with("CCTUI_DISPATCH")));

        let undispatched = apply(&lean_pod());
        let gp = &undispatched.spec.as_ref().unwrap().init_containers.as_ref().unwrap()[1];
        assert!(gp.env.is_none(), "no dispatch annotation => no dispatch env");
    }

    #[test]
    fn other_containers_are_byte_identical() {
        let before = full_stack_pod();
        let after = apply(&before);

        let db_before = serde_json::to_value(worker(&before, "db")).unwrap();
        let db_after = serde_json::to_value(worker(&after, "db")).unwrap();
        assert_eq!(db_before, db_after, "passthrough container mutated");

        let migrate_before = serde_json::to_value(
            &before.spec.as_ref().unwrap().init_containers.as_ref().unwrap()[0],
        )
        .unwrap();
        let after_init = after.spec.as_ref().unwrap().init_containers.as_ref().unwrap();
        let migrate_after =
            serde_json::to_value(after_init.iter().find(|c| c.name == "migrate").unwrap()).unwrap();
        assert_eq!(migrate_before, migrate_after, "passthrough initContainer mutated");
    }

    #[test]
    fn guard_identity_annotation_sets_env() {
        let mut pod = lean_pod();
        pod.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(ANNOTATION_GUARD_IDENTITY.to_owned(), "release-bot".to_owned());
        let out = apply(&pod);
        let gp = &out.spec.as_ref().unwrap().init_containers.as_ref().unwrap()[1];
        let id =
            gp.env.as_ref().unwrap().iter().find(|e| e.name == "GUARD_PROXY_IDENTITY").unwrap();
        assert_eq!(id.value.as_deref(), Some("release-bot"));
    }

    fn rendered_gate(human_gate: Option<HumanGate>, guard: Option<&GuardSpec>) -> Vec<String> {
        let flags = ProfileFlags { human_gate, ..ProfileFlags::default() };
        let MutationOutcome::Patch(patch) = mutate_pod(&lean_pod(), IMG, guard, flags).unwrap()
        else {
            panic!("eligible pod must yield a patch");
        };
        let mut doc = serde_json::to_value(lean_pod()).unwrap();
        json_patch::patch(&mut doc, &patch.0).unwrap();
        let out: Pod = serde_json::from_value(doc).unwrap();
        let inits = out.spec.unwrap().init_containers.unwrap();
        let gp = inits.into_iter().find(|c| c.name == GUARD_PROXY_CONTAINER).unwrap();
        gp.env
            .into_iter()
            .flatten()
            .filter(|e| e.name == GUARD_ENV_HUMAN_GATE)
            .map(|e| e.value.unwrap_or_default())
            .collect()
    }

    #[test]
    fn declared_human_gate_reaches_the_guard_proxy_over_guard_env() {
        let guard: GuardSpec = serde_json::from_value(serde_json::json!({
            "env": [{ "name": GUARD_ENV_HUMAN_GATE, "value": "bogus" }]
        }))
        .unwrap();
        assert_eq!(rendered_gate(Some(HumanGate::DRAFT_PR_ONLY), Some(&guard)), ["draftPrOnly"]);
        assert_eq!(rendered_gate(Some(HumanGate::NO_PUSH), None), ["noPush"]);
        assert_eq!(
            rendered_gate(Some(HumanGate::OUTWARD_HELD_FOR_APPROVAL), None),
            ["outwardHeldForApproval"]
        );
        let custom = HumanGate::new("reviewWithScreenshots");
        assert_eq!(rendered_gate(custom, Some(&guard)), ["reviewWithScreenshots"]);
        assert!(rendered_gate(None, None).is_empty(), "no gate renders nothing");
    }

    #[test]
    fn gpg_annotation_wires_gpg_agent() {
        let mut pod = lean_pod();
        pod.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(ANNOTATION_GPG_SIGNING.to_owned(), "true".to_owned());
        let out = apply(&pod);
        let spec = out.spec.as_ref().unwrap();
        assert!(spec.volumes.as_ref().unwrap().iter().any(|v| v.name == "gpg-agent"));

        let gp = &spec.init_containers.as_ref().unwrap()[1];
        assert!(gp.volume_mounts.as_ref().unwrap().iter().any(|m| m.name == "gpg-agent"));

        let w = worker(&out, "worker");
        assert!(w.volume_mounts.as_ref().unwrap().iter().any(|m| m.name == "gpg-agent"));
    }

    #[test]
    fn no_gpg_means_no_gpg_agent() {
        let out = apply(&lean_pod());
        let spec = out.spec.as_ref().unwrap();
        assert!(!spec.volumes.as_ref().unwrap().iter().any(|v| v.name == "gpg-agent"));
        let gp = &spec.init_containers.as_ref().unwrap()[1];
        assert!(!gp.volume_mounts.as_ref().unwrap().iter().any(|m| m.name == "gpg-agent"));
    }

    #[test]
    fn reinjection_is_a_noop() {
        let once = apply(&lean_pod());
        assert!(is_injected(&once));
        assert!(
            matches!(
                mutate_pod(&once, IMG, None, ProfileFlags::default()).unwrap(),
                MutationOutcome::AlreadyInjected
            ),
            "second pass must not double-inject"
        );
    }

    #[test]
    fn pod_without_label_is_untouched() {
        let mut pod = lean_pod();
        pod.metadata.labels = None;
        assert!(!is_eligible(&pod));
        assert!(matches!(
            mutate_pod(&pod, IMG, None, ProfileFlags::default()).unwrap(),
            MutationOutcome::NotEligible
        ));
    }

    fn init<'a>(pod: &'a Pod, name: &str) -> &'a Container {
        let inits = pod.spec.as_ref().unwrap().init_containers.as_ref().unwrap();
        inits.iter().find(|c| c.name == name).unwrap()
    }

    fn env_pairs(c: &Container) -> Vec<(&str, Option<&str>)> {
        c.env.iter().flatten().map(|e| (e.name.as_str(), e.value.as_deref())).collect()
    }

    #[test]
    fn net_init_renders_the_profile_resolver_and_payload_env_cannot_override_it() {
        let mut pod = lean_pod();
        pod.spec.as_mut().unwrap().containers[0].env = Some(vec![EnvVar {
            name: NET_INIT_ENV_DNS_RESOLVER.to_owned(),
            value: Some("8.8.8.8".to_owned()),
            value_from: None,
        }]);
        let guard: GuardSpec = serde_json::from_value(serde_json::json!({
            "env": [{ "name": "WORKER_DNS_RESOLVER", "value": "1.1.1.1" }],
            "envFrom": [{ "configMapRef": { "name": "attacker-env" } }]
        }))
        .unwrap();
        let dns_resolver = Some(" 10.43.0.53 ".to_owned());
        let flags = ProfileFlags { dns_resolver, ..ProfileFlags::default() };
        let out = apply_flags(&pod, Some(&guard), flags);
        let net_init = init(&out, NET_INIT_CONTAINER);
        assert_eq!(env_pairs(net_init), [(NET_INIT_ENV_DNS_RESOLVER, Some("10.43.0.53"))]);
        assert!(net_init.env_from.is_none(), "no env source can shadow the resolver");
        assert_eq!(
            env_pairs(worker(&out, "worker"))[0],
            (NET_INIT_ENV_DNS_RESOLVER, Some("8.8.8.8")),
            "the worker's own env stays the worker's"
        );
    }

    #[test]
    fn unset_or_blank_resolver_renders_the_default() {
        for dns_resolver in [None, Some(String::new()), Some("   ".to_owned())] {
            let flags = ProfileFlags { dns_resolver, ..ProfileFlags::default() };
            let out = apply_flags(&lean_pod(), None, flags);
            assert_eq!(
                env_pairs(init(&out, NET_INIT_CONTAINER)),
                [(NET_INIT_ENV_DNS_RESOLVER, Some(DEFAULT_DNS_RESOLVER))]
            );
        }
    }

    #[test]
    fn sidecars_carry_ephemeral_storage_requests_and_limits() {
        let out = apply(&lean_pod());
        for (name, want) in [
            (NET_INIT_CONTAINER, golden_net_init_resources()),
            (GUARD_PROXY_CONTAINER, golden_guard_proxy_resources()),
        ] {
            let have = serde_json::to_value(init(&out, name).resources.as_ref().unwrap()).unwrap();
            assert_eq!(have, want, "{name}");
        }
    }

    #[test]
    fn profile_sidecar_resources_override_the_defaults_key_by_key() {
        let overrides: SidecarResources = serde_json::from_value(serde_json::json!({
            "guardProxy": {
                "requests": { "ephemeral-storage": "64Mi" },
                "limits": { "ephemeral-storage": "1Gi", "memory": "512Mi" }
            },
            "netInit": { "limits": { "cpu": "200m" } }
        }))
        .unwrap();
        let flags = ProfileFlags { sidecar_resources: Some(overrides), ..ProfileFlags::default() };
        let out = apply_flags(&lean_pod(), None, flags);
        let rendered =
            |name: &str| serde_json::to_value(init(&out, name).resources.as_ref()).unwrap();
        assert_eq!(
            rendered(GUARD_PROXY_CONTAINER),
            serde_json::json!({
                "requests": { "cpu": "25m", "memory": "32Mi", "ephemeral-storage": "64Mi" },
                "limits": { "cpu": "500m", "memory": "512Mi", "ephemeral-storage": "1Gi" }
            })
        );
        assert_eq!(
            rendered(NET_INIT_CONTAINER),
            serde_json::json!({
                "requests": { "cpu": "10m", "memory": "16Mi", "ephemeral-storage": "4Mi" },
                "limits": { "cpu": "200m", "memory": "64Mi", "ephemeral-storage": "16Mi" }
            })
        );
        let w = worker(&out, "worker");
        assert!(w.resources.is_none(), "sidecar overrides never reach the worker");
    }

    #[test]
    fn custom_worker_container_name_honored() {
        let mut pod = lean_pod();
        pod.spec.as_mut().unwrap().containers[0].name = "agent".to_owned();
        pod.metadata
            .annotations
            .as_mut()
            .unwrap()
            .insert(ANNOTATION_WORKER_CONTAINER.to_owned(), "agent".to_owned());
        let out = apply(&pod);
        let w = worker(&out, "agent");
        assert_eq!(w.security_context.as_ref().unwrap().run_as_user, Some(0));
    }
}
