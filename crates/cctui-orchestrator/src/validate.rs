//! Validating admission webhook: guardrails, not adversarial defense.
//!
//! The profile author (operator) is trusted; these checks catch accidental
//! sandbox breakage, malformed profiles, and the one agent-influenced surface —
//! the dispatch request. Validating webhooks run *after* every mutating webhook,
//! so the pod inspected here already carries the injected envelope.
//!
//! Three families of check run on pods carrying the `cctui.dev/worker-profile`
//! label (pods without it are allowed untouched):
//! 1. structural — exactly one worker container and the envelope marker present;
//! 2. sandbox-defeating footguns on the worker/pod;
//! 3. profile conformance — the pod is exactly what the dispatcher + mutating
//!    webhook would legitimately produce from the named profile.

use crate::envelope::{
    GUARD_PROXY_CONTAINER, NET_INIT_CONTAINER, PROXY_UID, VOL_GPG_AGENT, VOL_GUARD_PROXY_CA,
    VOL_GUARD_PROXY_INJECT, VOL_GUARD_STATE, VOL_HOME, VOL_OVERLAY, VOL_PROXY_POLICY,
    WORKER_ADDED_CAPS,
};
use crate::{
    ANNOTATION_ENVELOPE_INJECTED, ANNOTATION_WORKER_CONTAINER, DEFAULT_WORKER_CONTAINER,
    LABEL_WORKER_PROFILE, WorkerProfileSpec,
};
use k8s_openapi::api::core::v1::{Container, Pod, PodSpec};
use std::collections::BTreeSet;

const SECRET_REF_PREFIXES: [&str; 3] = ["vault:", "bao:", "k8s:"];

/// Outcome of validating a single pod.
pub enum Decision {
    Allow,
    Deny(String),
}

/// Fetches the `WorkerProfile` a pod was instantiated from. Abstracted so unit
/// tests use an in-memory map while `main` wires an `Api<WorkerProfile>`.
#[async_trait::async_trait]
pub trait ProfileSource: Send + Sync {
    /// The profile's spec, or `None` if it does not exist. `Err` is a lookup
    /// failure (denies fail-closed with the message).
    async fn get(&self, namespace: &str, name: &str) -> anyhow::Result<Option<WorkerProfileSpec>>;
}

fn is_eligible(pod: &Pod) -> bool {
    pod.metadata.labels.as_ref().is_some_and(|l| l.contains_key(LABEL_WORKER_PROFILE))
}

fn profile_name(pod: &Pod) -> Option<&str> {
    pod.metadata.labels.as_ref().and_then(|l| l.get(LABEL_WORKER_PROFILE)).map(String::as_str)
}

fn worker_container_name(pod: &Pod) -> &str {
    pod.metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(ANNOTATION_WORKER_CONTAINER))
        .map_or(DEFAULT_WORKER_CONTAINER, String::as_str)
}

fn is_injected(pod: &Pod) -> bool {
    pod.metadata
        .annotations
        .as_ref()
        .is_some_and(|a| a.get(ANNOTATION_ENVELOPE_INJECTED).map(String::as_str) == Some("true"))
}

/// Validate a pod against the guardrails, resolving its profile via `source`.
pub async fn validate(pod: &Pod, namespace: &str, source: &dyn ProfileSource) -> Decision {
    if !is_eligible(pod) {
        return Decision::Allow;
    }
    match validate_inner(pod, namespace, source).await {
        Ok(()) => Decision::Allow,
        Err(msg) => Decision::Deny(msg),
    }
}

async fn validate_inner(
    pod: &Pod,
    namespace: &str,
    source: &dyn ProfileSource,
) -> Result<(), String> {
    let worker_name = worker_container_name(pod).to_owned();
    let spec = pod.spec.as_ref().ok_or_else(|| "profiled pod has no spec".to_owned())?;

    let worker = check_structure(pod, spec, &worker_name)?;
    check_sandbox(spec, worker, &worker_name)?;

    let name = profile_name(pod).ok_or_else(|| "profiled pod missing profile label".to_owned())?;
    let profile = source.get(namespace, name).await.map_err(|e| {
        format!("cannot resolve WorkerProfile `{name}` in `{namespace}`: {e} (denied fail-closed)")
    })?;
    let profile = profile.ok_or_else(|| {
        format!("WorkerProfile `{name}` not found in `{namespace}` — refusing to admit a pod whose profile is missing")
    })?;

    check_conformance(spec, worker, &worker_name, &profile)
}

/// Exactly one worker container, and the envelope marker present (fail-closed
/// pairing with the mutating webhook).
fn check_structure<'a>(
    pod: &Pod,
    spec: &'a PodSpec,
    worker_name: &str,
) -> Result<&'a Container, String> {
    let matches: Vec<&Container> =
        spec.containers.iter().filter(|c| c.name == worker_name).collect();
    match matches.len() {
        0 => {
            return Err(format!(
                "profiled pod names no `{worker_name}` worker container (the \
                 `cctui.dev/worker-container` convention)"
            ));
        }
        1 => {}
        n => {
            return Err(format!(
                "profiled pod has {n} containers named `{worker_name}`; exactly one worker is \
                 required"
            ));
        }
    }
    if !is_injected(pod) {
        return Err(format!(
            "profiled pod is missing the `{ANNOTATION_ENVELOPE_INJECTED}: \"true\"` marker — the \
             mutating webhook did not inject the envelope; refusing to admit an unsandboxed worker"
        ));
    }
    Ok(matches[0])
}

/// Reject sandbox-defeating footguns on the worker container / pod.
fn check_sandbox(spec: &PodSpec, worker: &Container, worker_name: &str) -> Result<(), String> {
    if spec.host_pid == Some(true) {
        return Err("pod sets hostPID: true — breaks worker isolation".to_owned());
    }
    if spec.host_ipc == Some(true) {
        return Err("pod sets hostIPC: true — breaks worker isolation".to_owned());
    }
    if spec.host_network == Some(true) {
        return Err(
            "pod sets hostNetwork: true — worker egress must go through the guard-proxy".to_owned()
        );
    }

    if let Some(sc) = &worker.security_context {
        if sc.privileged == Some(true) {
            return Err(format!(
                "worker container `{worker_name}` is privileged: true — a privileged worker \
                 escapes the sandbox"
            ));
        }
        if sc.run_as_user == Some(PROXY_UID) {
            return Err(format!(
                "worker container `{worker_name}` runs as uid {PROXY_UID} — that is the \
                 guard-proxy identity the worker must never assume"
            ));
        }
        if let Some(caps) = &sc.capabilities
            && let Some(add) = &caps.add
        {
            let sanctioned: BTreeSet<&str> = WORKER_ADDED_CAPS.iter().copied().collect();
            if let Some(extra) = add.iter().find(|c| !sanctioned.contains(c.as_str())) {
                return Err(format!(
                    "worker container `{worker_name}` adds capability `{extra}` beyond the \
                     sanctioned envelope set {WORKER_ADDED_CAPS:?}"
                ));
            }
        }
    }

    let host_path_vols: BTreeSet<&str> = spec
        .volumes
        .iter()
        .flatten()
        .filter(|v| v.host_path.is_some())
        .map(|v| v.name.as_str())
        .collect();
    if !host_path_vols.is_empty()
        && let Some(mount) =
            worker.volume_mounts.iter().flatten().find(|m| host_path_vols.contains(m.name.as_str()))
    {
        return Err(format!(
            "worker container `{worker_name}` mounts hostPath volume `{}` — host paths must not \
             reach the worker",
            mount.name
        ));
    }
    Ok(())
}

/// The pod is exactly what the dispatcher + mutating webhook produce from
/// `profile`: no raw pod-spec overrides smuggled through the dispatch request.
fn check_conformance(
    spec: &PodSpec,
    worker: &Container,
    worker_name: &str,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    if spec.service_account_name != profile.service_account_name {
        return Err(format!(
            "serviceAccountName `{}` does not match the profile's `{}` — the dispatch must not \
             override the worker identity / secret scope",
            spec.service_account_name.as_deref().unwrap_or("<none>"),
            profile.service_account_name.as_deref().unwrap_or("<none>"),
        ));
    }
    if worker.name != *profile.worker_container_name() {
        return Err(format!(
            "worker container is named `{}` but the profile declares `{}`",
            worker.name,
            profile.worker_container_name()
        ));
    }
    if worker.image.as_deref() != Some(profile.image.as_str()) {
        return Err(format!(
            "worker image `{}` does not match the profile's `{}`",
            worker.image.as_deref().unwrap_or("<none>"),
            profile.image
        ));
    }
    if worker.command != profile.command {
        return Err("worker command does not match the profile".to_owned());
    }
    if worker.args != profile.args {
        return Err("worker args do not match the profile".to_owned());
    }
    if spec.node_selector != profile.node_selector {
        return Err("nodeSelector does not match the profile".to_owned());
    }
    if spec.runtime_class_name != profile.runtime_class_name {
        return Err("runtimeClassName does not match the profile".to_owned());
    }

    check_name_sets(spec, worker_name, profile)?;
    check_volumes(spec, profile)?;
    check_worker_env(worker, worker_name, profile)
}

fn check_name_sets(
    spec: &PodSpec,
    worker_name: &str,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    let mut want_containers: BTreeSet<&str> = BTreeSet::new();
    want_containers.insert(worker_name);
    for c in profile.containers.iter().flatten() {
        want_containers.insert(c.name.as_str());
    }
    let have_containers: BTreeSet<&str> = spec.containers.iter().map(|c| c.name.as_str()).collect();
    diff_names("container", &want_containers, &have_containers)?;

    let mut want_inits: BTreeSet<&str> = BTreeSet::new();
    want_inits.insert(NET_INIT_CONTAINER);
    want_inits.insert(GUARD_PROXY_CONTAINER);
    for c in profile.init_containers.iter().flatten() {
        want_inits.insert(c.name.as_str());
    }
    let have_inits: BTreeSet<&str> =
        spec.init_containers.iter().flatten().map(|c| c.name.as_str()).collect();
    diff_names("initContainer", &want_inits, &have_inits)
}

fn check_volumes(spec: &PodSpec, profile: &WorkerProfileSpec) -> Result<(), String> {
    let mut want: BTreeSet<&str> = [
        VOL_HOME,
        VOL_OVERLAY,
        VOL_GUARD_STATE,
        VOL_PROXY_POLICY,
        VOL_GUARD_PROXY_INJECT,
        VOL_GUARD_PROXY_CA,
    ]
    .into_iter()
    .collect();
    if profile.gpg_signing {
        want.insert(VOL_GPG_AGENT);
    }
    for v in profile.volumes.iter().flatten() {
        want.insert(v.name.as_str());
    }
    let have: BTreeSet<&str> = spec.volumes.iter().flatten().map(|v| v.name.as_str()).collect();
    diff_names("volume", &want, &have)
}

fn diff_names(kind: &str, want: &BTreeSet<&str>, have: &BTreeSet<&str>) -> Result<(), String> {
    if let Some(extra) = have.difference(want).next() {
        return Err(format!(
            "unexpected {kind} `{extra}` — not part of the profile or the injected envelope (raw \
             override rejected)"
        ));
    }
    if let Some(missing) = want.difference(have).next() {
        return Err(format!("expected {kind} `{missing}` is absent from the pod"));
    }
    Ok(())
}

/// Payload env is agent-influenced by design, so env *names* are not policed.
/// Instead: reject `valueFrom` refs the profile itself does not declare (no
/// mounting cluster secrets via env), and reject secret-ref-shaped literals.
fn check_worker_env(
    worker: &Container,
    worker_name: &str,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    for env in worker.env.iter().flatten() {
        if let Some(value_from) = &env.value_from {
            let declared = profile
                .env
                .iter()
                .flatten()
                .any(|p| p.name == env.name && p.value_from.as_ref() == Some(value_from));
            if !declared {
                return Err(format!(
                    "worker env `{}` uses a valueFrom reference not present in the profile — the \
                     dispatch must not mount cluster secrets into `{worker_name}`",
                    env.name
                ));
            }
        }
        if let Some(value) = &env.value
            && let Some(prefix) = SECRET_REF_PREFIXES.iter().find(|p| value.starts_with(**p))
        {
            return Err(format!(
                "worker env `{}` is a secret-ref-shaped literal (`{prefix}…`) — secret refs are \
                 resolved by the guard-proxy, never passed through worker env",
                env.name
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::{self, EnvelopeConfig, default_sidecar_image};
    use std::collections::HashMap;

    struct MapSource(HashMap<String, WorkerProfileSpec>);

    #[async_trait::async_trait]
    impl ProfileSource for MapSource {
        async fn get(
            &self,
            _namespace: &str,
            name: &str,
        ) -> anyhow::Result<Option<WorkerProfileSpec>> {
            Ok(self.0.get(name).cloned())
        }
    }

    fn source(name: &str, profile: WorkerProfileSpec) -> MapSource {
        let mut m = HashMap::new();
        m.insert(name.to_owned(), profile);
        MapSource(m)
    }

    fn lean_profile() -> WorkerProfileSpec {
        serde_json::from_value(serde_json::json!({
            "image": "registry.example.com/worker:latest",
            "serviceAccountName": "worker-lean",
            "env": [{ "name": "LOG_LEVEL", "value": "info" }],
        }))
        .unwrap()
    }

    /// Instantiate a pod the way `spawn.rs` does, then apply the mutating
    /// envelope — a faithful, admissible pod.
    fn dispatched_pod(profile_name: &str, profile: &WorkerProfileSpec) -> Pod {
        let worker_name = profile.worker_container_name();
        let mut worker = serde_json::Map::new();
        worker.insert("name".into(), serde_json::json!(worker_name));
        worker.insert("image".into(), serde_json::json!(profile.image));
        if let Some(command) = &profile.command {
            worker.insert("command".into(), serde_json::to_value(command).unwrap());
        }
        if let Some(args) = &profile.args {
            worker.insert("args".into(), serde_json::to_value(args).unwrap());
        }
        let mut env: Vec<serde_json::Value> =
            profile.env.iter().flatten().map(|e| serde_json::to_value(e).unwrap()).collect();
        env.push(serde_json::json!({ "name": "SESSION_ID", "value": "s1" }));
        env.push(serde_json::json!({ "name": "TASK_ID", "value": "s1" }));
        worker.insert("env".into(), serde_json::Value::Array(env));

        let mut containers = vec![serde_json::Value::Object(worker)];
        for c in profile.containers.iter().flatten() {
            containers.push(serde_json::to_value(c).unwrap());
        }

        let mut pod_spec = serde_json::Map::new();
        pod_spec.insert("restartPolicy".into(), serde_json::json!("Never"));
        pod_spec.insert("containers".into(), serde_json::Value::Array(containers));
        if let Some(init) = &profile.init_containers {
            pod_spec.insert("initContainers".into(), serde_json::to_value(init).unwrap());
        }
        if let Some(volumes) = &profile.volumes {
            pod_spec.insert("volumes".into(), serde_json::to_value(volumes).unwrap());
        }
        if let Some(ns) = &profile.node_selector {
            pod_spec.insert("nodeSelector".into(), serde_json::to_value(ns).unwrap());
        }
        if let Some(rc) = &profile.runtime_class_name {
            pod_spec.insert("runtimeClassName".into(), serde_json::json!(rc));
        }
        if let Some(sa) = &profile.service_account_name {
            pod_spec.insert("serviceAccountName".into(), serde_json::json!(sa));
        }

        let mut annotations = serde_json::Map::new();
        annotations.insert(ANNOTATION_WORKER_CONTAINER.into(), serde_json::json!(worker_name));
        if profile.gpg_signing {
            annotations.insert(crate::ANNOTATION_GPG_SIGNING.into(), serde_json::json!("true"));
        }

        let pod: Pod = serde_json::from_value(serde_json::json!({
            "apiVersion": "v1",
            "kind": "Pod",
            "metadata": {
                "name": "worker-pod",
                "namespace": "workers",
                "labels": { LABEL_WORKER_PROFILE: profile_name },
                "annotations": annotations,
            },
            "spec": pod_spec,
        }))
        .unwrap();

        let cfg = EnvelopeConfig {
            sidecar_image: default_sidecar_image(),
            worker_container: worker_name.to_owned(),
            guard_identity: None,
            gpg: profile.gpg_signing,
        };
        envelope::inject(&pod, &cfg)
    }

    fn worker_mut(pod: &mut Pod) -> &mut Container {
        let name = worker_container_name(pod).to_owned();
        pod.spec.as_mut().unwrap().containers.iter_mut().find(|c| c.name == name).unwrap()
    }

    async fn decide(pod: &Pod, src: &dyn ProfileSource) -> Decision {
        validate(pod, "workers", src).await
    }

    fn deny_msg(d: Decision) -> String {
        match d {
            Decision::Deny(m) => m,
            Decision::Allow => panic!("expected Deny, got Allow"),
        }
    }

    #[tokio::test]
    async fn faithful_dispatched_pod_is_allowed() {
        let pod = dispatched_pod("lean", &lean_profile());
        let src = source("lean", lean_profile());
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
    }

    #[tokio::test]
    async fn full_stack_dispatched_pod_is_allowed() {
        let profile: WorkerProfileSpec = serde_json::from_value(serde_json::json!({
            "image": "registry.example.com/worker:latest",
            "command": ["/entrypoint"],
            "args": ["--serve"],
            "workerContainer": "agent",
            "serviceAccountName": "worker-full",
            "gpgSigning": true,
            "runtimeClassName": "gvisor",
            "nodeSelector": { "kubernetes.io/arch": "amd64" },
            "env": [{
                "name": "REGISTRY_TOKEN",
                "valueFrom": { "secretKeyRef": { "name": "reg", "key": "token" } }
            }],
            "volumes": [{ "name": "db-data", "emptyDir": {} }],
            "initContainers": [{ "name": "migrate", "image": "registry.example.com/m:1" }],
            "containers": [{ "name": "db", "image": "registry.example.com/postgres:16" }],
        }))
        .unwrap();
        let pod = dispatched_pod("full", &profile);
        let src = source("full", profile);
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
    }

    #[tokio::test]
    async fn unlabeled_pod_is_allowed() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        pod.metadata.labels = None;
        let src = source("lean", lean_profile());
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
    }

    #[tokio::test]
    async fn uninjected_profiled_pod_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        pod.metadata.annotations.as_mut().unwrap().remove(ANNOTATION_ENVELOPE_INJECTED);
        let src = source("lean", lean_profile());
        let msg = deny_msg(decide(&pod, &src).await);
        assert!(msg.contains("marker"), "{msg}");
    }

    #[tokio::test]
    async fn missing_profile_is_denied() {
        let pod = dispatched_pod("lean", &lean_profile());
        let src = MapSource(HashMap::new());
        let msg = deny_msg(decide(&pod, &src).await);
        assert!(msg.contains("not found"), "{msg}");
    }

    #[tokio::test]
    async fn privileged_worker_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).security_context.as_mut().unwrap().privileged = Some(true);
        let src = source("lean", lean_profile());
        let msg = deny_msg(decide(&pod, &src).await);
        assert!(msg.contains("privileged"), "{msg}");
    }

    #[tokio::test]
    async fn host_pid_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        pod.spec.as_mut().unwrap().host_pid = Some(true);
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("hostPID"));
    }

    #[tokio::test]
    async fn worker_running_as_proxy_uid_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).security_context.as_mut().unwrap().run_as_user = Some(PROXY_UID);
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("1337"));
    }

    #[tokio::test]
    async fn extra_capability_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod)
            .security_context
            .as_mut()
            .unwrap()
            .capabilities
            .as_mut()
            .unwrap()
            .add
            .as_mut()
            .unwrap()
            .push("NET_RAW".to_owned());
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("NET_RAW"));
    }

    #[tokio::test]
    async fn hostpath_mounted_into_worker_is_denied() {
        use k8s_openapi::api::core::v1::{HostPathVolumeSource, Volume, VolumeMount};
        let mut pod = dispatched_pod("lean", &lean_profile());
        let spec = pod.spec.as_mut().unwrap();
        spec.volumes.get_or_insert_with(Vec::new).push(Volume {
            name: "hostroot".to_owned(),
            host_path: Some(HostPathVolumeSource { path: "/".to_owned(), type_: None }),
            ..Volume::default()
        });
        let worker = worker_mut(&mut pod);
        worker.volume_mounts.get_or_insert_with(Vec::new).push(VolumeMount {
            name: "hostroot".to_owned(),
            mount_path: "/host".to_owned(),
            ..VolumeMount::default()
        });
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("hostPath"));
    }

    #[tokio::test]
    async fn service_account_override_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        pod.spec.as_mut().unwrap().service_account_name = Some("privileged-sa".to_owned());
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("serviceAccountName"));
    }

    #[tokio::test]
    async fn extra_container_raw_override_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        pod.spec.as_mut().unwrap().containers.push(Container {
            name: "sneaky".to_owned(),
            image: Some("registry.example.com/x:1".to_owned()),
            ..Container::default()
        });
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("sneaky"));
    }

    #[tokio::test]
    async fn extra_volume_raw_override_is_denied() {
        use k8s_openapi::api::core::v1::{EmptyDirVolumeSource, Volume};
        let mut pod = dispatched_pod("lean", &lean_profile());
        pod.spec.as_mut().unwrap().volumes.get_or_insert_with(Vec::new).push(Volume {
            name: "smuggled".to_owned(),
            empty_dir: Some(EmptyDirVolumeSource::default()),
            ..Volume::default()
        });
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("smuggled"));
    }

    #[tokio::test]
    async fn env_valuefrom_not_in_profile_is_denied() {
        use k8s_openapi::api::core::v1::{EnvVar, EnvVarSource, SecretKeySelector};
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).env.get_or_insert_with(Vec::new).push(EnvVar {
            name: "STOLEN".to_owned(),
            value: None,
            value_from: Some(EnvVarSource {
                secret_key_ref: Some(SecretKeySelector {
                    name: "cluster-secret".to_owned(),
                    key: "token".to_owned(),
                    optional: None,
                }),
                ..EnvVarSource::default()
            }),
        });
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("valueFrom"));
    }

    #[tokio::test]
    async fn secret_ref_shaped_literal_env_is_denied() {
        use k8s_openapi::api::core::v1::EnvVar;
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).env.get_or_insert_with(Vec::new).push(EnvVar {
            name: "TOKEN".to_owned(),
            value: Some("vault:secret/data/ci#gh".to_owned()),
            value_from: None,
        });
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("secret-ref"));
    }
}
