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
    ANNOTATION_ENVELOPE_INJECTED, ANNOTATION_GPG_SIGNING, ANNOTATION_GUARD_IDENTITY,
    ANNOTATION_WORKER_CONTAINER, DEFAULT_WORKER_CONTAINER, GuardSpec, LABEL_WORKER_PROFILE,
};
use k8s_openapi::api::core::v1::{
    AppArmorProfile, Capabilities, ConfigMapEnvSource, ConfigMapProjection, ConfigMapVolumeSource,
    Container, DownwardAPIProjection, DownwardAPIVolumeFile, EmptyDirVolumeSource, EnvFromSource,
    EnvVar, HTTPGetAction, KeyToPath, ObjectFieldSelector, Pod, PodSecurityContext, Probe,
    ProjectedVolumeSource, ResourceRequirements, SecurityContext, ServiceAccountTokenProjection,
    Volume, VolumeMount, VolumeProjection,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use std::collections::BTreeMap;

pub const NET_INIT_CONTAINER: &str = "net-init";
pub const GUARD_PROXY_CONTAINER: &str = "guard-proxy";
const CONFIGMAP_GUARD_PROXY_ENV: &str = "guard-proxy-env";
const CONFIGMAP_GUARD_PROXY_INJECT: &str = "guard-proxy-inject";

pub const VOL_HOME: &str = "home";
pub const VOL_OVERLAY: &str = "overlay";
pub const VOL_GUARD_STATE: &str = "guard-state";
pub const VOL_PROXY_POLICY: &str = "proxy-policy";
pub const VOL_GUARD_PROXY_INJECT: &str = "guard-proxy-inject";
pub const VOL_GUARD_PROXY_CA: &str = "guard-proxy-ca";
pub const VOL_GUARD_PROXY_SA_TOKEN: &str = "guard-proxy-sa-token";
pub const VOL_GPG_AGENT: &str = "gpg-agent";

/// Where the kubelet would mount the pod's service-account token. Pods set
/// `automountServiceAccountToken: false`; only the guard-proxy (Vault
/// Kubernetes login, `k8s:` secret refs) gets an equivalent projection.
const SA_TOKEN_DIR: &str = "/var/run/secrets/kubernetes.io/serviceaccount";
const SA_TOKEN_EXPIRATION_SECONDS: i64 = 3600;
const CONFIGMAP_KUBE_ROOT_CA: &str = "kube-root-ca.crt";

/// Volume names the envelope mounts into the worker container, always. Mirrors
/// [`sandbox_worker`]; [`VOL_GPG_AGENT`] is added on top only under `gpgSigning`.
pub const WORKER_ENVELOPE_MOUNTS: [&str; 5] =
    [VOL_HOME, VOL_OVERLAY, VOL_GUARD_STATE, VOL_PROXY_POLICY, VOL_GUARD_PROXY_CA];

pub const PROXY_UID: i64 = 1337;
const FS_GROUP: i64 = 1000;

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

/// Everything the envelope construction needs, resolved from the pod's
/// admission-time label/annotations.
pub struct EnvelopeConfig {
    pub sidecar_image: String,
    pub worker_container: String,
    pub guard_identity: Option<String>,
    pub gpg: bool,
    pub guard: Option<GuardSpec>,
}

impl EnvelopeConfig {
    fn from_pod(pod: &Pod, sidecar_image: &str, guard: Option<&GuardSpec>) -> Self {
        let annotations = pod.metadata.annotations.as_ref();
        let get = |key: &str| annotations.and_then(|a| a.get(key)).map(String::as_str);
        Self {
            sidecar_image: sidecar_image.to_owned(),
            worker_container: get(ANNOTATION_WORKER_CONTAINER)
                .unwrap_or(DEFAULT_WORKER_CONTAINER)
                .to_owned(),
            guard_identity: get(ANNOTATION_GUARD_IDENTITY).map(ToOwned::to_owned),
            gpg: get(ANNOTATION_GPG_SIGNING) == Some("true"),
            guard: guard.cloned(),
        }
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
/// `guard` is the source profile's `spec.guard`. A serialization failure is an `Err` so the admission webhook
/// fails closed (deny) instead of admitting an unpatched, unsandboxed worker.
pub fn mutate_pod(
    pod: &Pod,
    sidecar_image: &str,
    guard: Option<&GuardSpec>,
) -> anyhow::Result<MutationOutcome> {
    if !is_eligible(pod) {
        return Ok(MutationOutcome::NotEligible);
    }
    if is_injected(pod) {
        return Ok(MutationOutcome::AlreadyInjected);
    }
    let cfg = EnvelopeConfig::from_pod(pod, sidecar_image, guard);
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
    worker.security_context = Some(SecurityContext {
        run_as_user: Some(0),
        app_armor_profile: Some(AppArmorProfile {
            type_: "Unconfined".to_owned(),
            localhost_profile: None,
        }),
        capabilities: Some(Capabilities {
            drop: Some(vec!["ALL".to_owned()]),
            add: Some(WORKER_ADDED_CAPS.iter().map(|s| (*s).to_owned()).collect()),
        }),
        ..SecurityContext::default()
    });

    let env = worker.env.get_or_insert_with(Vec::new);
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
    for m in wanted {
        if !mounts.iter().any(|x| x.name == m.name) {
            mounts.push(m);
        }
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
        resources: Some(resources("10m", "16Mi", "100m", "64Mi")),
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
        security_context: Some(SecurityContext {
            run_as_user: Some(PROXY_UID),
            run_as_group: Some(PROXY_UID),
            allow_privilege_escalation: Some(false),
            capabilities: Some(Capabilities { drop: Some(vec!["ALL".to_owned()]), add: None }),
            ..SecurityContext::default()
        }),
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
        resources: Some(resources("25m", "32Mi", "500m", "256Mi")),
        ..Container::default()
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
    volumes
}

/// The same projection the kubelet automounts: token, cluster CA, namespace.
fn sa_token_volume() -> Volume {
    let sources = vec![
        VolumeProjection {
            service_account_token: Some(ServiceAccountTokenProjection {
                path: "token".to_owned(),
                expiration_seconds: Some(SA_TOKEN_EXPIRATION_SECONDS),
                audience: None,
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
                }]),
            }),
            ..VolumeProjection::default()
        },
    ];
    Volume {
        name: VOL_GUARD_PROXY_SA_TOKEN.to_owned(),
        projected: Some(ProjectedVolumeSource { sources: Some(sources), default_mode: None }),
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

fn resources(cpu_req: &str, mem_req: &str, cpu_lim: &str, mem_lim: &str) -> ResourceRequirements {
    let mut requests = BTreeMap::new();
    requests.insert("cpu".to_owned(), Quantity(cpu_req.to_owned()));
    requests.insert("memory".to_owned(), Quantity(mem_req.to_owned()));
    let mut limits = BTreeMap::new();
    limits.insert("cpu".to_owned(), Quantity(cpu_lim.to_owned()));
    limits.insert("memory".to_owned(), Quantity(mem_lim.to_owned()));
    ResourceRequirements {
        requests: Some(requests),
        limits: Some(limits),
        ..ResourceRequirements::default()
    }
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
        let patch = match mutate_pod(pod, IMG, guard).expect("mutate_pod must not error") {
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

    fn golden_resources(cr: &str, mr: &str, cl: &str, ml: &str) -> serde_json::Value {
        serde_json::json!({
            "requests": { "cpu": cr, "memory": mr },
            "limits": { "cpu": cl, "memory": ml }
        })
    }

    fn golden_guard_proxy() -> serde_json::Value {
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
            "securityContext": {
                "runAsUser": 1337,
                "runAsGroup": 1337,
                "allowPrivilegeEscalation": false,
                "capabilities": { "drop": ["ALL"] }
            },
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
            "resources": golden_resources("25m", "32Mi", "500m", "256Mi")
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
                    {
                        "name": "net-init",
                        "image": IMG,
                        "command": ["cctui-worker-net-init"],
                        "securityContext": {
                            "runAsUser": 0,
                            "capabilities": { "drop": ["ALL"], "add": ["NET_ADMIN"] }
                        },
                        "resources": golden_resources("10m", "16Mi", "100m", "64Mi")
                    },
                    golden_guard_proxy()
                ],
                "containers": [golden_worker()],
                "securityContext": { "fsGroup": 1000 },
                "volumes": golden_volumes()
            }
        })
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
        let token = sources.iter().find_map(|s| s.service_account_token.as_ref()).unwrap();
        assert_eq!(token.path, "token");
        assert_eq!(token.expiration_seconds, Some(3600));
        assert!(token.audience.is_none(), "default audience: same token the kubelet mounts");
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
            matches!(mutate_pod(&once, IMG, None).unwrap(), MutationOutcome::AlreadyInjected),
            "second pass must not double-inject"
        );
    }

    #[test]
    fn pod_without_label_is_untouched() {
        let mut pod = lean_pod();
        pod.metadata.labels = None;
        assert!(!is_eligible(&pod));
        assert!(matches!(mutate_pod(&pod, IMG, None).unwrap(), MutationOutcome::NotEligible));
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
