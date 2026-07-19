//! Pure construction of the secretless-worker envelope injected into profiled
//! pods, plus the [`mutate_pod`] entry point that turns an admitted pod into a
//! `JSONPatch`.
//!
//! Only the worker container is sandboxed; every other container and
//! initContainer is passthrough (threat model: the operator is trusted, the
//! agent in the worker container is not). Environment-specific guard-proxy
//! settings are supplied at runtime via the `guard-proxy-env` `ConfigMap`
//! (`envFrom`), never baked into the binary.

use crate::{
    ANNOTATION_ENVELOPE_INJECTED, ANNOTATION_GPG_SIGNING, ANNOTATION_GUARD_IDENTITY,
    ANNOTATION_WORKER_CONTAINER, DEFAULT_WORKER_CONTAINER, LABEL_WORKER_PROFILE,
};
use k8s_openapi::api::core::v1::{
    AppArmorProfile, Capabilities, ConfigMapEnvSource, ConfigMapVolumeSource, Container,
    EmptyDirVolumeSource, EnvFromSource, EnvVar, HTTPGetAction, Pod, PodSecurityContext, Probe,
    ResourceRequirements, SecurityContext, Volume, VolumeMount,
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
pub const VOL_GPG_AGENT: &str = "gpg-agent";

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
}

impl EnvelopeConfig {
    fn from_pod(pod: &Pod, sidecar_image: &str) -> Self {
        let annotations = pod.metadata.annotations.as_ref();
        let get = |key: &str| annotations.and_then(|a| a.get(key)).map(String::as_str);
        Self {
            sidecar_image: sidecar_image.to_owned(),
            worker_container: get(ANNOTATION_WORKER_CONTAINER)
                .unwrap_or(DEFAULT_WORKER_CONTAINER)
                .to_owned(),
            guard_identity: get(ANNOTATION_GUARD_IDENTITY).map(ToOwned::to_owned),
            gpg: get(ANNOTATION_GPG_SIGNING) == Some("true"),
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

/// Compute the `JSONPatch` that injects the envelope, or `None` for a no-op
/// (pod not eligible, already injected, or nothing to change).
#[must_use]
pub fn mutate_pod(pod: &Pod, sidecar_image: &str) -> Option<json_patch::Patch> {
    if !is_eligible(pod) || is_injected(pod) {
        return None;
    }
    let cfg = EnvelopeConfig::from_pod(pod, sidecar_image);
    let injected = inject(pod, &cfg);
    let before = serde_json::to_value(pod).ok()?;
    let after = serde_json::to_value(&injected).ok()?;
    let patch = json_patch::diff(&before, &after);
    if patch.0.is_empty() { None } else { Some(patch) }
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

    upsert_env(worker.env.get_or_insert_with(Vec::new), "WORKER_NET_MODE", "transparent-external");

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

    let mut mounts = vec![
        mount(VOL_PROXY_POLICY, "/var/run/guard-proxy", false),
        mount(VOL_GUARD_PROXY_INJECT, "/etc/guard-proxy", true),
        mount(VOL_GUARD_PROXY_CA, "/var/run/guard-proxy-ca", false),
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
        env_from: Some(vec![EnvFromSource {
            config_map_ref: Some(ConfigMapEnvSource {
                name: CONFIGMAP_GUARD_PROXY_ENV.to_owned(),
                optional: Some(false),
            }),
            ..EnvFromSource::default()
        }]),
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
    ];
    if cfg.gpg {
        volumes.push(empty_dir(VOL_GPG_AGENT));
    }
    volumes
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
        let patch = mutate_pod(pod, IMG).expect("eligible pod yields a patch");
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

        let gp = &init[1];
        assert_eq!(
            gp.env_from.as_ref().unwrap()[0].config_map_ref.as_ref().unwrap().name,
            "guard-proxy-env"
        );
        assert!(gp.env.is_none(), "no identity annotation -> ConfigMap default only");
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
        assert!(mutate_pod(&once, IMG).is_none(), "second pass must not double-inject");
    }

    #[test]
    fn pod_without_label_is_untouched() {
        let mut pod = lean_pod();
        pod.metadata.labels = None;
        assert!(!is_eligible(&pod));
        assert!(mutate_pod(&pod, IMG).is_none());
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
