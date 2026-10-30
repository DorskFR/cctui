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
//!    webhook would legitimately produce from the named profile, the
//!    profile's `spec.guard` only tunes the guard-proxy within its rules, and
//!    nothing in the profile contradicts its `spec.posture`.

use crate::envelope::{
    GUARD_PROXY_CONTAINER, NET_INIT_CONTAINER, PROXY_UID, VOL_GPG_AGENT, VOL_GUARD_PROXY_CA,
    VOL_GUARD_PROXY_INJECT, VOL_GUARD_PROXY_SA_TOKEN, VOL_GUARD_STATE, VOL_HOME, VOL_OVERLAY,
    VOL_PROXY_POLICY, WARM_REPO_DIR_ENV, WORKER_UID, has_overlay, worker_added_caps,
    worker_envelope_mounts,
};
use crate::{
    ANNOTATION_ENV_SECRET, ANNOTATION_ENVELOPE_INJECTED, ANNOTATION_POSTURE,
    ANNOTATION_WORKER_CONTAINER, DEFAULT_WORKER_CONTAINER, GuardSpec, LABEL_WORKER_PROFILE,
    Posture, WorkerProfileSpec,
};
use k8s_openapi::api::core::v1::{
    Container, Pod, PodSpec, PodTemplateSpec, SecurityContext, Toleration,
};
use std::collections::{BTreeMap, BTreeSet};

const SECRET_REF_PREFIXES: [&str; 4] = ["vault:", "bao:", "k8s:", "aws-sm:"];

const GUARD_ENV_PREFIX: &str = "GUARD_PROXY_";
const GUARD_ENV_IDENTITY: &str = "GUARD_PROXY_IDENTITY";
const GUARD_ENV_ALLOW_PRIVATE_IPS: &str = "GUARD_PROXY_ALLOW_PRIVATE_IPS";
const GUARD_ENV_PRIVATE_ALLOW: &str = "GUARD_PROXY_PRIVATE_ALLOW";

const APPARMOR_ANNOTATION_PREFIX: &str = "container.apparmor.security.beta.kubernetes.io/";

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
    let annotations = pod.metadata.annotations.as_ref();
    let posture = annotated_posture(annotations)?;
    check_sandbox(spec, worker, &worker_name, posture)?;
    if posture == Posture::Nonroot {
        check_nonroot_worker(worker, &worker_name, annotations)?;
    }

    let name = profile_name(pod).ok_or_else(|| "profiled pod missing profile label".to_owned())?;
    let profile = source.get(namespace, name).await.map_err(|e| {
        format!("cannot resolve WorkerProfile `{name}` in `{namespace}`: {e} (denied fail-closed)")
    })?;
    let profile = profile.ok_or_else(|| {
        format!("WorkerProfile `{name}` not found in `{namespace}` — refusing to admit a pod whose profile is missing")
    })?;

    if let Some(guard) = &profile.guard {
        check_guard(guard).map_err(|e| format!("WorkerProfile `{name}` spec.guard: {e}"))?;
    }
    check_posture(&profile).map_err(|e| format!("WorkerProfile `{name}` spec.posture: {e}"))?;
    check_posture_annotation(annotations, &profile)?;
    check_conformance(spec, worker, &worker_name, &profile, dispatch_env_secret(pod))
}

fn annotated_posture(annotations: Option<&BTreeMap<String, String>>) -> Result<Posture, String> {
    let value = annotations.and_then(|a| a.get(ANNOTATION_POSTURE)).map(String::as_str);
    Posture::from_annotation(value).ok_or_else(|| {
        format!(
            "pod annotation `{ANNOTATION_POSTURE}: {}` names no posture (want `nonroot`, or no \
             annotation for `root`)",
            value.unwrap_or_default()
        )
    })
}

/// The pod (or Job template) was rendered for the profile's current posture.
fn check_posture_annotation(
    annotations: Option<&BTreeMap<String, String>>,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    let have = annotated_posture(annotations)?;
    let want = profile.posture();
    if have != want {
        return Err(format!(
            "pod was rendered for posture `{}` but the profile's posture is `{}` — it was built \
             before the profile changed",
            have.as_str(),
            want.as_str()
        ));
    }
    Ok(())
}

/// Check that nothing in a profile contradicts its `spec.posture`. `root`
/// adds no rule.
pub fn check_posture(profile: &WorkerProfileSpec) -> Result<(), String> {
    if profile.posture() == Posture::Root {
        return Ok(());
    }
    check_no_overlay(profile)?;
    for c in profile.init_containers.iter().flatten().chain(profile.containers.iter().flatten()) {
        check_unprivileged(c)?;
    }
    Ok(())
}

fn check_no_overlay(profile: &WorkerProfileSpec) -> Result<(), String> {
    if profile.env.iter().flatten().any(|e| e.name == WARM_REPO_DIR_ENV) {
        return Err(format!(
            "nonroot: env `{WARM_REPO_DIR_ENV}` selects the overlayfs warm cache, which needs \
             `SYS_ADMIN` — bake the warm repo into the image (`/workspace/.warm-baked`) instead, \
             or use posture root"
        ));
    }
    if profile.volumes.iter().flatten().any(|v| v.name == VOL_OVERLAY) {
        return Err(format!(
            "nonroot: volume `{VOL_OVERLAY}` is the root posture's overlay emptyDir — nonroot has \
             no overlay"
        ));
    }
    if let Some(m) = profile
        .volume_mounts
        .iter()
        .flatten()
        .find(|m| m.name == VOL_OVERLAY || m.mount_path.trim_end_matches('/') == "/overlay")
    {
        return Err(format!(
            "nonroot: volumeMount `{}` at `{}` is the root posture's overlay — nonroot has no \
             overlay",
            m.name, m.mount_path
        ));
    }
    Ok(())
}

/// Under nonroot, `net-init` stays the pod's only privileged container.
fn check_unprivileged(c: &Container) -> Result<(), String> {
    let Some(sc) = &c.security_context else { return Ok(()) };
    let name = &c.name;
    if sc.privileged == Some(true) {
        return Err(format!(
            "nonroot: container `{name}` is privileged: true — net-init is the only privileged \
             container"
        ));
    }
    if let Some(cap) =
        sc.capabilities.as_ref().and_then(|caps| caps.add.as_ref()).and_then(|a| a.first())
    {
        return Err(format!(
            "nonroot: container `{name}` adds capability `{cap}` — net-init is the only container \
             with added capabilities"
        ));
    }
    if sc.run_as_user == Some(0) {
        return Err(format!(
            "nonroot: container `{name}` sets runAsUser: 0 — net-init is the only container that \
             runs as root"
        ));
    }
    Ok(())
}

/// The nonroot worker runs with the envelope's restricted `securityContext`.
/// Fields the envelope leaves unset may be API-server defaulted, so they are
/// only checked for weakening values, not compared exactly.
fn check_nonroot_worker(
    worker: &Container,
    worker_name: &str,
    annotations: Option<&BTreeMap<String, String>>,
) -> Result<(), String> {
    let apparmor_key = format!("{APPARMOR_ANNOTATION_PREFIX}{worker_name}");
    if annotations
        .and_then(|a| a.get(&apparmor_key))
        .is_some_and(|v| v.eq_ignore_ascii_case("unconfined"))
    {
        return Err(format!(
            "pod annotation `{apparmor_key}: unconfined` lifts AppArmor from the nonroot worker"
        ));
    }
    if let Some(why) = nonroot_violation(worker.security_context.as_ref()) {
        return Err(format!(
            "worker container `{worker_name}` securityContext is not the nonroot envelope's: {why}"
        ));
    }
    Ok(())
}

fn nonroot_violation(sc: Option<&SecurityContext>) -> Option<&'static str> {
    let Some(sc) = sc else { return Some("no securityContext") };
    let keeps_capabilities = !sc
        .capabilities
        .as_ref()
        .and_then(|c| c.drop.as_ref())
        .is_some_and(|d| d.iter().any(|c| c == "ALL"));
    if sc.run_as_user != Some(WORKER_UID) {
        Some("runAsUser must be 1000")
    } else if sc.run_as_group != Some(WORKER_UID) {
        Some("runAsGroup must be 1000")
    } else if sc.run_as_non_root != Some(true) {
        Some("runAsNonRoot must be true")
    } else if sc.allow_privilege_escalation != Some(false) {
        Some("allowPrivilegeEscalation must be false")
    } else if keeps_capabilities {
        Some("capabilities must drop ALL")
    } else if sc.seccomp_profile.as_ref().map(|p| p.type_.as_str()) != Some("RuntimeDefault") {
        Some("seccompProfile must be RuntimeDefault")
    } else if sc.app_armor_profile.as_ref().is_some_and(|p| p.type_ == "Unconfined") {
        Some("appArmorProfile must not be Unconfined")
    } else if sc.se_linux_options.is_some() {
        Some("seLinuxOptions must be unset")
    } else if sc.proc_mount.as_deref().is_some_and(|m| m != "Default") {
        Some("procMount must be Default")
    } else {
        None
    }
}

/// Check a profile's `spec.guard`.
///
/// `spec.guard.env` may only set `GUARD_PROXY_*` (never the per-dispatch
/// identity), and widening private egress needs `allowPrivateEgress: true`.
/// Narrowing (zeroing either setting) needs nothing. `envFrom` contents are
/// not visible at admission and stay operator-owned, like `guard-proxy-env`.
pub fn check_guard(guard: &GuardSpec) -> Result<(), String> {
    for env in guard.env.iter().flatten() {
        if !env.name.starts_with(GUARD_ENV_PREFIX) {
            return Err(format!(
                "env `{}` is not a `{GUARD_ENV_PREFIX}*` setting — spec.guard only configures \
                 the guard-proxy",
                env.name
            ));
        }
        if env.name == GUARD_ENV_IDENTITY {
            return Err(format!(
                "env `{GUARD_ENV_IDENTITY}` is set per dispatch (`cctui.dev/guard-identity`), \
                 not per profile"
            ));
        }
        if !guard.allow_private_egress && widens_private_egress(env) {
            return Err(format!(
                "env `{}` widens private egress — set spec.guard.allowPrivateEgress: true to \
                 make that explicit",
                env.name
            ));
        }
    }
    Ok(())
}

/// A `valueFrom` on a widening setting can't be read at admission, so it
/// counts as widening.
fn widens_private_egress(env: &k8s_openapi::api::core::v1::EnvVar) -> bool {
    let value = env.value.as_deref().unwrap_or_default().trim();
    match env.name.as_str() {
        GUARD_ENV_ALLOW_PRIVATE_IPS => {
            env.value_from.is_some()
                || matches!(value.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on")
        }
        GUARD_ENV_PRIVATE_ALLOW => env.value_from.is_some() || !value.is_empty(),
        _ => false,
    }
}

/// The pod's own per-dispatch env Secret: named by its annotation and bound to
/// the Job that owns the pod, so a dispatch cannot point it at another Secret.
fn dispatch_env_secret(pod: &Pod) -> Option<&str> {
    let secret = pod.metadata.annotations.as_ref()?.get(ANNOTATION_ENV_SECRET)?;
    let labels = pod.metadata.labels.as_ref()?;
    let job = labels.get("batch.kubernetes.io/job-name").or_else(|| labels.get("job-name"))?;
    secret
        .strip_prefix(&crate::env_secret_name(job, ""))
        .is_some_and(|nonce| !nonce.is_empty())
        .then_some(secret.as_str())
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
fn check_sandbox(
    spec: &PodSpec,
    worker: &Container,
    worker_name: &str,
    posture: Posture,
) -> Result<(), String> {
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
            let sanctioned = worker_added_caps(posture);
            if let Some(extra) = add.iter().find(|c| !sanctioned.contains(&c.as_str())) {
                return Err(format!(
                    "worker container `{worker_name}` adds capability `{extra}` beyond the \
                     sanctioned envelope set {sanctioned:?} of posture {}",
                    posture.as_str()
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
    dispatch_secret: Option<&str>,
) -> Result<(), String> {
    check_common_template_conformance(spec, worker, profile)?;
    check_tolerations(spec, profile, true)?;
    check_name_sets(spec, worker_name, profile)?;
    check_volumes(spec, profile)?;
    check_worker_mounts(worker, worker_name, profile)?;
    check_worker_env(worker, worker_name, profile, dispatch_secret)
}

/// The conformance decided entirely by the Job's pod *template*.
///
/// These are the fields the dispatcher fixes at creation, before the envelope is
/// injected. The kube dispatcher calls this to spot a queued Job whose template
/// no longer matches its profile; the field list lives here so the dispatcher's
/// notion of "conformant" cannot drift from the webhook's.
pub fn check_template_conformance(
    spec: &PodSpec,
    worker: &Container,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    check_common_template_conformance(spec, worker, profile)?;
    check_tolerations(spec, profile, false)
}

fn check_common_template_conformance(
    spec: &PodSpec,
    worker: &Container,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    let worker_name = worker.name.as_str();
    if spec.automount_service_account_token != Some(false) {
        return Err(
            "pod does not set automountServiceAccountToken: false — the worker must not receive \
             the service-account token (the guard-proxy gets its own projection)"
                .to_owned(),
        );
    }
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

    check_worker_env_from(worker, worker_name, profile)
}

/// Kubernetes defaults an omitted operator to Equal; empty optional strings
/// and list ordering do not change the scheduling permission.
fn same_toleration(have: &Toleration, want: &Toleration) -> bool {
    have.key.as_deref().unwrap_or_default() == want.key.as_deref().unwrap_or_default()
        && have.operator.as_deref().unwrap_or("Equal")
            == want.operator.as_deref().unwrap_or("Equal")
        && have.value.as_deref().unwrap_or_default() == want.value.as_deref().unwrap_or_default()
        && have.effect.as_deref().unwrap_or_default() == want.effect.as_deref().unwrap_or_default()
        && have.toleration_seconds == want.toleration_seconds
}

/// Only Pods get admission defaults. Never permit these additions on a Job
/// template, where they would hide profile drift. Custom admission additions
/// belong in the operator's profile.
fn standard_admission_toleration(t: &Toleration, declared: &[Toleration]) -> bool {
    // The API server skips a node-health default when the profile already
    // covers that taint. An additional, longer duration would widen the policy.
    if declared.iter().any(|expected| {
        (expected.key.as_deref().unwrap_or_default().is_empty() || expected.key == t.key)
            && (expected.effect.as_deref().unwrap_or_default().is_empty()
                || expected.effect == t.effect)
    }) {
        return false;
    }
    if t.operator.as_deref() != Some("Exists") || !t.value.as_deref().unwrap_or_default().is_empty()
    {
        return false;
    }
    match (t.key.as_deref(), t.effect.as_deref()) {
        (
            Some("node.kubernetes.io/not-ready" | "node.kubernetes.io/unreachable"),
            Some("NoExecute"),
        ) => {
            // DefaultTolerationSeconds is configurable on the API server.
            t.toleration_seconds.is_some_and(|seconds| seconds >= 0)
        }
        (Some("node.kubernetes.io/memory-pressure"), Some("NoSchedule")) => {
            t.toleration_seconds.is_none()
        }
        _ => false,
    }
}

fn check_tolerations(
    spec: &PodSpec,
    profile: &WorkerProfileSpec,
    admitted_pod: bool,
) -> Result<(), String> {
    let have = spec.tolerations.as_deref().unwrap_or_default();
    let want = profile.tolerations.as_deref().unwrap_or_default();
    if want.iter().any(|expected| !have.iter().any(|actual| same_toleration(actual, expected))) {
        return Err("tolerations are missing an entry from the profile".to_owned());
    }
    if have.iter().any(|actual| {
        !(want.iter().any(|expected| same_toleration(actual, expected))
            || admitted_pod && standard_admission_toleration(actual, want))
    }) {
        return Err("tolerations include an entry not allowed by the profile".to_owned());
    }
    Ok(())
}

/// Why a Job's immutable pod template no longer conforms to `profile`, or
/// `None` if it still does.
#[must_use]
pub fn template_drift(template: &PodTemplateSpec, profile: &WorkerProfileSpec) -> Option<String> {
    let spec = template.spec.as_ref()?;
    let worker_name = template
        .metadata
        .as_ref()
        .and_then(|m| m.annotations.as_ref())
        .and_then(|a| a.get(ANNOTATION_WORKER_CONTAINER))
        .map_or(DEFAULT_WORKER_CONTAINER, String::as_str);
    let worker = spec.containers.iter().find(|c| c.name == worker_name)?;
    check_template_conformance(spec, worker, profile)
        .and_then(|()| {
            check_posture_annotation(
                template.metadata.as_ref().and_then(|m| m.annotations.as_ref()),
                profile,
            )
        })
        .err()
}

/// Worker `envFrom` must be exactly the profile's — the envelope never adds
/// `envFrom` to the worker, so any extra source is a raw dispatch override.
fn check_worker_env_from(
    worker: &Container,
    worker_name: &str,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    if worker.env_from != profile.env_from {
        return Err(format!(
            "worker container `{worker_name}` envFrom does not match the profile — the dispatch \
             must not add env sources to the worker"
        ));
    }
    Ok(())
}

/// Worker volumeMounts must be exactly the profile's mounts ∪ the envelope's
/// worker mounts (plus the gpg-agent mount under `gpgSigning`). An extra mount
/// is a raw override; a missing one means the envelope didn't land.
fn check_worker_mounts(
    worker: &Container,
    worker_name: &str,
    profile: &WorkerProfileSpec,
) -> Result<(), String> {
    let mut want: BTreeSet<&str> = worker_envelope_mounts(profile.posture()).into_iter().collect();
    if profile.gpg_signing {
        want.insert(VOL_GPG_AGENT);
    }
    for m in profile.volume_mounts.iter().flatten() {
        want.insert(m.name.as_str());
    }
    let have: BTreeSet<&str> = worker
        .volume_mounts
        .iter()
        .flatten()
        .map(|m| m.name.as_str())
        .filter(|n| !is_api_server_injected(n))
        .collect();
    if let Some(extra) = have.difference(&want).next() {
        return Err(format!(
            "worker container `{worker_name}` mounts `{extra}` — not part of the profile or the \
             injected envelope (raw override rejected)"
        ));
    }
    if let Some(missing) = want.difference(&have).next() {
        return Err(format!(
            "worker container `{worker_name}` is missing expected volumeMount `{missing}`"
        ));
    }
    Ok(())
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
        VOL_GUARD_PROXY_SA_TOKEN,
    ]
    .into_iter()
    .collect();
    if !has_overlay(profile.posture()) {
        want.remove(VOL_OVERLAY);
    }
    if profile.gpg_signing {
        want.insert(VOL_GPG_AGENT);
    }
    for v in profile.volumes.iter().flatten() {
        want.insert(v.name.as_str());
    }
    let have: BTreeSet<&str> = spec
        .volumes
        .iter()
        .flatten()
        .map(|v| v.name.as_str())
        .filter(|n| !is_api_server_injected(n))
        .collect();
    diff_names("volume", &want, &have)
}

/// Projected SA-token volume/mounts the API server itself adds during
/// admission — exempt from raw-override rejection.
fn is_api_server_injected(name: &str) -> bool {
    name.starts_with("kube-api-access-")
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
/// mounting cluster secrets via env) other than the pod's own per-dispatch env
/// Secret, and reject secret-ref-shaped literals.
fn check_worker_env(
    worker: &Container,
    worker_name: &str,
    profile: &WorkerProfileSpec,
    dispatch_secret: Option<&str>,
) -> Result<(), String> {
    for env in worker.env.iter().flatten() {
        if let Some(value_from) = &env.value_from {
            // fieldRef/resourceFieldRef reach only the pod's own metadata, so
            // they stay exempt from the cluster-data smuggling check.
            let smuggled = match (&value_from.secret_key_ref, &value_from.config_map_key_ref) {
                (Some(sk), _) if dispatch_secret == Some(sk.name.as_str()) => sk.key != env.name,
                (Some(sk), _) => !profile.env.iter().flatten().any(|p| {
                    p.name == env.name
                        && p.value_from
                            .as_ref()
                            .and_then(|v| v.secret_key_ref.as_ref())
                            .is_some_and(|d| d.name == sk.name && d.key == sk.key)
                }),
                (None, Some(ck)) => !profile.env.iter().flatten().any(|p| {
                    p.name == env.name
                        && p.value_from
                            .as_ref()
                            .and_then(|v| v.config_map_key_ref.as_ref())
                            .is_some_and(|d| d.name == ck.name && d.key == ck.key)
                }),
                (None, None) => false,
            };
            if smuggled {
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
        if let Some(env_from) = &profile.env_from {
            worker.insert("envFrom".into(), serde_json::to_value(env_from).unwrap());
        }
        if let Some(mounts) = &profile.volume_mounts {
            worker.insert("volumeMounts".into(), serde_json::to_value(mounts).unwrap());
        }

        let mut containers = vec![serde_json::Value::Object(worker)];
        for c in profile.containers.iter().flatten() {
            containers.push(serde_json::to_value(c).unwrap());
        }

        let mut pod_spec = serde_json::Map::new();
        pod_spec.insert("restartPolicy".into(), serde_json::json!("Never"));
        pod_spec.insert("automountServiceAccountToken".into(), serde_json::json!(false));
        pod_spec.insert("containers".into(), serde_json::Value::Array(containers));
        if let Some(init) = &profile.init_containers {
            pod_spec.insert("initContainers".into(), serde_json::to_value(init).unwrap());
        }
        if let Some(volumes) = &profile.volumes {
            pod_spec.insert("volumes".into(), serde_json::to_value(volumes).unwrap());
        }
        if let Some(tolerations) = &profile.tolerations {
            pod_spec.insert("tolerations".into(), serde_json::to_value(tolerations).unwrap());
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
        for (k, v) in profile.pod_annotations.iter().flatten() {
            annotations.insert(k.clone(), serde_json::json!(v));
        }
        annotations.insert(ANNOTATION_WORKER_CONTAINER.into(), serde_json::json!(worker_name));
        if profile.gpg_signing {
            annotations.insert(crate::ANNOTATION_GPG_SIGNING.into(), serde_json::json!("true"));
        }
        if let Some(posture) = profile.posture().annotation() {
            annotations.insert(ANNOTATION_POSTURE.into(), serde_json::json!(posture));
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
            guard: profile.guard.clone(),
            posture: profile.posture(),
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
    async fn a_pod_that_automounts_the_sa_token_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        pod.spec.as_mut().unwrap().automount_service_account_token = None;
        let src = source("lean", lean_profile());
        let msg = deny_msg(decide(&pod, &src).await);
        assert!(msg.contains("automountServiceAccountToken"), "{msg}");
    }

    #[tokio::test]
    async fn api_server_injected_sa_token_volume_is_tolerated() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        let spec = pod.spec.as_mut().unwrap();
        spec.volumes.get_or_insert_default().push(k8s_openapi::api::core::v1::Volume {
            name: "kube-api-access-hw82n".into(),
            ..Default::default()
        });
        for c in &mut spec.containers {
            c.volume_mounts.get_or_insert_default().push(k8s_openapi::api::core::v1::VolumeMount {
                name: "kube-api-access-hw82n".into(),
                mount_path: "/var/run/secrets/kubernetes.io/serviceaccount".into(),
                ..Default::default()
            });
        }
        let src = source("lean", lean_profile());
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
    }

    #[test]
    fn template_tolerations_ignore_order_and_defaults_but_detect_missing_entries() {
        let profile: WorkerProfileSpec = serde_json::from_value(serde_json::json!({
            "image": "example.com/worker:latest",
            "tolerations": [
                {"key": "dedicated", "value": "cctui-worker", "effect": "NoSchedule"},
                {"key": "node.kubernetes.io/unreachable", "operator": "Exists", "effect": "NoExecute", "tolerationSeconds": 30}
            ]
        })).unwrap();
        let mut spec = dispatched_pod("tainted", &profile).spec.unwrap();
        let entries = spec.tolerations.as_mut().unwrap();
        entries[0].operator = Some("Equal".into());
        entries.reverse();
        assert!(check_tolerations(&spec, &profile, false).is_ok());
        spec.tolerations.as_mut().unwrap().remove(0);
        assert!(check_tolerations(&spec, &profile, true).unwrap_err().contains("missing"));

        let empty = lean_profile();
        spec.tolerations = Some(vec![]);
        assert!(check_tolerations(&spec, &empty, false).is_ok());
        spec.tolerations = Some(
            serde_json::from_value(serde_json::json!([{
                "key": "node.kubernetes.io/not-ready", "operator": "Exists",
                "effect": "NoExecute", "tolerationSeconds": 300
            }]))
            .unwrap(),
        );
        assert!(check_tolerations(&spec, &empty, true).is_ok());
        assert!(check_tolerations(&spec, &empty, false).is_err());
    }

    #[tokio::test]
    async fn tolerations_cannot_be_added_or_broadened_outside_the_profile() {
        let profile: WorkerProfileSpec = serde_json::from_value(serde_json::json!({
            "image": "example.com/worker:latest",
            "tolerations": [{"key": "dedicated", "value": "cctui-worker", "effect": "NoSchedule"}]
        }))
        .unwrap();
        let mut pod = dispatched_pod("tainted", &profile);
        // Model the API server defaulting Equal and reordering the list.
        pod.spec.as_mut().unwrap().tolerations = Some(serde_json::from_value(serde_json::json!([
            {"key": "dedicated", "operator": "Equal", "value": "cctui-worker", "effect": "NoSchedule"}
        ])).unwrap());
        let src = source("tainted", profile);
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
        pod.spec.as_mut().unwrap().tolerations.as_mut().unwrap()[0].operator =
            Some("Exists".into());
        assert!(deny_msg(decide(&pod, &src).await).contains("tolerations"));
    }

    #[tokio::test]
    async fn admission_defaults_cannot_broaden_an_explicit_node_health_toleration() {
        let profile: WorkerProfileSpec = serde_json::from_value(serde_json::json!({
            "image": "example.com/worker:latest",
            "tolerations": [{"key": "node.kubernetes.io/unreachable", "operator": "Exists",
                "effect": "NoExecute", "tolerationSeconds": 30}]
        }))
        .unwrap();
        let mut pod = dispatched_pod("bounded", &profile);
        let src = source("bounded", profile);
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
        pod.spec.as_mut().unwrap().tolerations.as_mut().unwrap().push(
            serde_json::from_value(serde_json::json!({"key": "node.kubernetes.io/unreachable",
                "operator": "Exists", "effect": "NoExecute", "tolerationSeconds": 300}))
            .unwrap(),
        );
        assert!(deny_msg(decide(&pod, &src).await).contains("tolerations"));
    }

    #[tokio::test]
    async fn only_bounded_standard_admission_tolerations_are_allowed() {
        let src = source("lean", lean_profile());
        for (toleration, allowed) in [
            (
                serde_json::json!({"key": "node.kubernetes.io/not-ready", "operator": "Exists", "effect": "NoExecute", "tolerationSeconds": 300}),
                true,
            ),
            (
                serde_json::json!({"key": "node.kubernetes.io/unreachable", "operator": "Exists", "effect": "NoExecute", "tolerationSeconds": 30}),
                true,
            ),
            (
                serde_json::json!({"key": "node.kubernetes.io/memory-pressure", "operator": "Exists", "effect": "NoSchedule"}),
                true,
            ),
            (
                serde_json::json!({"key": "node.kubernetes.io/unreachable", "operator": "Exists", "effect": "NoExecute"}),
                false,
            ),
            (
                serde_json::json!({"key": "dedicated", "operator": "Exists", "effect": "NoSchedule"}),
                false,
            ),
            (serde_json::json!({"operator": "Exists"}), false),
        ] {
            let mut pod = dispatched_pod("lean", &lean_profile());
            pod.spec.as_mut().unwrap().tolerations =
                Some(serde_json::from_value(serde_json::json!([toleration])).unwrap());
            assert_eq!(
                matches!(decide(&pod, &src).await, Decision::Allow),
                allowed,
                "{toleration}"
            );
        }
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
            "envFrom": [{ "configMapRef": { "name": "worker-config" } }],
            "volumeMounts": [
                { "name": "logs", "mountPath": "/var/log/worker" },
                { "name": "shim", "mountPath": "/usr/local/bin/shim.sh", "subPath": "shim.sh" }
            ],
            "podAnnotations": { "example.dev/role": "worker" },
            "volumes": [
                { "name": "db-data", "emptyDir": {} },
                { "name": "logs", "emptyDir": {} },
                { "name": "shim", "configMap": { "name": "worker-shim" } }
            ],
            "initContainers": [{ "name": "migrate", "image": "registry.example.com/m:1" }],
            "containers": [{ "name": "db", "image": "registry.example.com/postgres:16" }],
        }))
        .unwrap();
        let pod = dispatched_pod("full", &profile);
        let src = source("full", profile);
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
    }

    #[tokio::test]
    async fn worker_env_from_not_in_profile_is_denied() {
        use k8s_openapi::api::core::v1::{ConfigMapEnvSource, EnvFromSource};
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).env_from.get_or_insert_with(Vec::new).push(EnvFromSource {
            config_map_ref: Some(ConfigMapEnvSource {
                name: "smuggled-config".to_owned(),
                optional: None,
            }),
            ..EnvFromSource::default()
        });
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("envFrom"));
    }

    #[tokio::test]
    async fn extra_worker_mount_is_denied() {
        use k8s_openapi::api::core::v1::VolumeMount;
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).volume_mounts.get_or_insert_with(Vec::new).push(VolumeMount {
            name: "smuggled".to_owned(),
            mount_path: "/smuggled".to_owned(),
            ..VolumeMount::default()
        });
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("smuggled"));
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

    fn with_dispatch_secret_ref(pod: &mut Pod, job: &str, secret: &str) {
        use k8s_openapi::api::core::v1::{EnvVar, EnvVarSource, SecretKeySelector};
        pod.metadata
            .labels
            .get_or_insert_with(Default::default)
            .insert("batch.kubernetes.io/job-name".to_owned(), job.to_owned());
        pod.metadata
            .annotations
            .get_or_insert_with(Default::default)
            .insert(ANNOTATION_ENV_SECRET.to_owned(), secret.to_owned());
        worker_mut(pod).env.get_or_insert_with(Vec::new).push(EnvVar {
            name: "CCTUI_MACHINE_KEY".to_owned(),
            value: None,
            value_from: Some(EnvVarSource {
                secret_key_ref: Some(SecretKeySelector {
                    name: secret.to_owned(),
                    key: "CCTUI_MACHINE_KEY".to_owned(),
                    optional: None,
                }),
                ..EnvVarSource::default()
            }),
        });
    }

    #[tokio::test]
    async fn own_dispatch_env_secret_ref_is_allowed() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        with_dispatch_secret_ref(&mut pod, "cctui-worker-abc", "cctui-worker-abc-env-123");
        let src = source("lean", lean_profile());
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
    }

    #[tokio::test]
    async fn dispatch_env_secret_of_another_job_is_denied() {
        let mut pod = dispatched_pod("lean", &lean_profile());
        with_dispatch_secret_ref(&mut pod, "cctui-worker-abc", "cluster-secret");
        let src = source("lean", lean_profile());
        assert!(deny_msg(decide(&pod, &src).await).contains("valueFrom"));
    }

    #[tokio::test]
    async fn downward_api_fieldref_env_is_allowed() {
        use k8s_openapi::api::core::v1::{EnvVar, EnvVarSource, ObjectFieldSelector};
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).env.get_or_insert_with(Vec::new).push(EnvVar {
            name: "POD_NAME".to_owned(),
            value: None,
            value_from: Some(EnvVarSource {
                field_ref: Some(ObjectFieldSelector {
                    api_version: Some("v1".to_owned()),
                    field_path: "metadata.name".to_owned(),
                }),
                ..EnvVarSource::default()
            }),
        });
        let src = source("lean", lean_profile());
        assert!(matches!(decide(&pod, &src).await, Decision::Allow));
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

    async fn decide_with_worker_env(value: &str) -> Decision {
        use k8s_openapi::api::core::v1::EnvVar;
        let mut pod = dispatched_pod("lean", &lean_profile());
        worker_mut(&mut pod).env.get_or_insert_with(Vec::new).push(EnvVar {
            name: "TOKEN".to_owned(),
            value: Some(value.to_owned()),
            value_from: None,
        });
        decide(&pod, &source("lean", lean_profile())).await
    }

    #[tokio::test]
    async fn every_guard_proxy_ref_scheme_is_denied_as_worker_env_literal() {
        for value in [
            "vault:secret/data/ci#gh",
            "bao:secret/data/ci#gh",
            "k8s:ns/secret#key",
            "aws-sm:cctui/worker/acme/github",
            "aws-sm:cctui/worker#token",
            "aws-sm:",
            "aws-sm:name#",
        ] {
            let msg = deny_msg(decide_with_worker_env(value).await);
            assert!(msg.contains("secret-ref"), "{value:?}: {msg}");
        }
    }

    async fn decide_with_guard(guard: serde_json::Value) -> Decision {
        let mut profile = lean_profile();
        profile.guard = Some(serde_json::from_value(guard).unwrap());
        let pod = dispatched_pod("lean", &profile);
        decide(&pod, &source("lean", profile)).await
    }

    #[tokio::test]
    async fn guard_env_and_env_from_overrides_are_allowed() {
        let decision = decide_with_guard(serde_json::json!({
            "envFrom": [{ "configMapRef": { "name": "guard-proxy-env-canary" } }],
            "env": [{ "name": "GUARD_PROXY_SECRET_TTL_SECS", "value": "60" }]
        }))
        .await;
        assert!(matches!(decision, Decision::Allow));
    }

    #[tokio::test]
    async fn guard_env_outside_guard_proxy_namespace_is_denied() {
        for name in ["LOG_LEVEL", "HTTPS_PROXY", "guard_proxy_allow_private_ips", "GUARD_PROXY"] {
            let msg = deny_msg(
                decide_with_guard(serde_json::json!({ "env": [{ "name": name, "value": "x" }] }))
                    .await,
            );
            assert!(msg.contains("spec.guard") && msg.contains(name), "{name}: {msg}");
        }
    }

    #[tokio::test]
    async fn guard_env_cannot_set_the_dispatch_identity() {
        let msg = deny_msg(
            decide_with_guard(serde_json::json!({
                "env": [{ "name": "GUARD_PROXY_IDENTITY", "value": "release-bot" }]
            }))
            .await,
        );
        assert!(msg.contains("per dispatch"), "{msg}");
    }

    #[tokio::test]
    async fn widening_private_egress_without_allow_private_egress_is_denied() {
        for env in [
            serde_json::json!({ "name": "GUARD_PROXY_ALLOW_PRIVATE_IPS", "value": "1" }),
            serde_json::json!({ "name": "GUARD_PROXY_ALLOW_PRIVATE_IPS", "value": " TRUE " }),
            serde_json::json!({ "name": "GUARD_PROXY_PRIVATE_ALLOW", "value": "git.lan:443" }),
            serde_json::json!({
                "name": "GUARD_PROXY_PRIVATE_ALLOW",
                "valueFrom": { "configMapKeyRef": { "name": "guard-extra", "key": "allow" } }
            }),
        ] {
            let msg = deny_msg(decide_with_guard(serde_json::json!({ "env": [env] })).await);
            assert!(msg.contains("allowPrivateEgress"), "{msg}");
        }
    }

    #[tokio::test]
    async fn widening_private_egress_with_allow_private_egress_is_allowed() {
        let decision = decide_with_guard(serde_json::json!({
            "allowPrivateEgress": true,
            "env": [
                { "name": "GUARD_PROXY_ALLOW_PRIVATE_IPS", "value": "1" },
                { "name": "GUARD_PROXY_PRIVATE_ALLOW", "value": "git.lan:443" }
            ]
        }))
        .await;
        assert!(matches!(decision, Decision::Allow));
    }

    #[tokio::test]
    async fn narrowing_private_egress_needs_no_flag() {
        for env in [
            serde_json::json!({ "name": "GUARD_PROXY_ALLOW_PRIVATE_IPS", "value": "0" }),
            serde_json::json!({ "name": "GUARD_PROXY_ALLOW_PRIVATE_IPS", "value": "" }),
            serde_json::json!({ "name": "GUARD_PROXY_ALLOW_PRIVATE_IPS" }),
            serde_json::json!({ "name": "GUARD_PROXY_PRIVATE_ALLOW", "value": "  " }),
        ] {
            let decision = decide_with_guard(serde_json::json!({ "env": [env.clone()] })).await;
            assert!(matches!(decision, Decision::Allow), "{env} must be allowed");
        }
    }

    fn nonroot_profile() -> WorkerProfileSpec {
        let mut profile = lean_profile();
        profile.posture = Some(Posture::Nonroot);
        profile
    }

    async fn decide_nonroot(mutate: impl FnOnce(&mut Pod)) -> Decision {
        let mut pod = dispatched_pod("lean", &nonroot_profile());
        mutate(&mut pod);
        decide(&pod, &source("lean", nonroot_profile())).await
    }

    #[tokio::test]
    async fn faithful_nonroot_pods_are_allowed() {
        assert!(matches!(decide_nonroot(|_| {}).await, Decision::Allow));

        let mut profile: WorkerProfileSpec = serde_json::from_value(serde_json::json!({
            "image": "registry.example.com/worker:latest",
            "posture": "nonroot",
            "gpgSigning": true,
            "volumes": [{ "name": "db-data", "emptyDir": {} }],
            "containers": [{
                "name": "db",
                "image": "registry.example.com/postgres:16",
                "securityContext": { "runAsUser": 999, "capabilities": { "drop": ["ALL"] } }
            }],
        }))
        .unwrap();
        let pod = dispatched_pod("full", &profile);
        assert!(matches!(decide(&pod, &source("full", profile.clone())).await, Decision::Allow));

        profile.posture = Some(Posture::Root);
        assert!(
            matches!(decide(&pod, &source("full", profile)).await, Decision::Deny(_)),
            "the same pod is not a root pod"
        );
    }

    #[tokio::test]
    async fn nonroot_worker_tolerates_api_server_defaults() {
        let decision = decide_nonroot(|pod| {
            worker_mut(pod).security_context.as_mut().unwrap().proc_mount =
                Some("Default".to_owned());
        })
        .await;
        assert!(matches!(decision, Decision::Allow));
    }

    #[tokio::test]
    async fn nonroot_worker_with_the_root_security_context_is_denied() {
        let msg = deny_msg(
            decide_nonroot(|pod| {
                worker_mut(pod).security_context =
                    Some(envelope::worker_security_context(Posture::Root));
            })
            .await,
        );
        assert!(msg.contains("SYS_ADMIN") && msg.contains("posture nonroot"), "{msg}");
    }

    #[tokio::test]
    async fn every_weakened_nonroot_worker_field_is_denied() {
        use k8s_openapi::api::core::v1::{AppArmorProfile, SELinuxOptions};
        type Edit = (&'static str, fn(&mut SecurityContext));
        let edits: [Edit; 10] = [
            ("runAsUser", |sc| sc.run_as_user = Some(0)),
            ("runAsGroup", |sc| sc.run_as_group = None),
            ("runAsNonRoot", |sc| sc.run_as_non_root = Some(false)),
            ("allowPrivilegeEscalation", |sc| sc.allow_privilege_escalation = Some(true)),
            ("drop ALL", |sc| sc.capabilities = None),
            ("RuntimeDefault", |sc| sc.seccomp_profile = None),
            ("Unconfined", |sc| {
                sc.app_armor_profile = Some(AppArmorProfile {
                    type_: "Unconfined".to_owned(),
                    localhost_profile: None,
                });
            }),
            ("seLinuxOptions", |sc| {
                sc.se_linux_options =
                    Some(SELinuxOptions { type_: Some("spc_t".to_owned()), ..Default::default() });
            }),
            ("procMount", |sc| sc.proc_mount = Some("Unmasked".to_owned())),
            ("privileged", |sc| sc.privileged = Some(true)),
        ];
        for (want, edit) in edits {
            let msg = deny_msg(
                decide_nonroot(|pod| edit(worker_mut(pod).security_context.as_mut().unwrap()))
                    .await,
            );
            assert!(msg.contains(want), "{want}: {msg}");
        }
        let msg = deny_msg(decide_nonroot(|pod| worker_mut(pod).security_context = None).await);
        assert!(msg.contains("no securityContext"), "{msg}");
    }

    #[tokio::test]
    async fn nonroot_worker_apparmor_annotation_is_denied() {
        let msg = deny_msg(
            decide_nonroot(|pod| {
                pod.metadata
                    .annotations
                    .as_mut()
                    .unwrap()
                    .insert(format!("{APPARMOR_ANNOTATION_PREFIX}worker"), "unconfined".to_owned());
            })
            .await,
        );
        assert!(msg.contains("AppArmor"), "{msg}");
    }

    #[tokio::test]
    async fn nonroot_pod_with_an_overlay_volume_is_denied() {
        use k8s_openapi::api::core::v1::{EmptyDirVolumeSource, Volume};
        let msg = deny_msg(
            decide_nonroot(|pod| {
                pod.spec.as_mut().unwrap().volumes.get_or_insert_with(Vec::new).push(Volume {
                    name: "overlay".to_owned(),
                    empty_dir: Some(EmptyDirVolumeSource::default()),
                    ..Volume::default()
                });
            })
            .await,
        );
        assert!(msg.contains("overlay"), "{msg}");
    }

    async fn nonroot_profile_denial(extra: serde_json::Value) -> String {
        let mut spec = serde_json::json!({
            "image": "registry.example.com/worker:latest",
            "posture": "nonroot",
        });
        if let serde_json::Value::Object(extra) = extra {
            spec.as_object_mut().unwrap().extend(extra);
        }
        let profile: WorkerProfileSpec = serde_json::from_value(spec).unwrap();
        let pod = dispatched_pod("p", &profile);
        deny_msg(decide(&pod, &source("p", profile)).await)
    }

    #[tokio::test]
    async fn nonroot_profiles_with_overlay_settings_are_denied() {
        let cases = [
            (
                serde_json::json!({ "env": [{ "name": "WARM_REPO_DIR", "value": "/warm" }] }),
                "WARM_REPO_DIR",
            ),
            (
                serde_json::json!({ "volumes": [{ "name": "overlay", "emptyDir": {} }] }),
                "volume `overlay`",
            ),
            (
                serde_json::json!({
                    "volumes": [{ "name": "scratch", "emptyDir": {} }],
                    "volumeMounts": [{ "name": "scratch", "mountPath": "/overlay/" }]
                }),
                "/overlay",
            ),
        ];
        for (extra, want) in cases {
            let msg = nonroot_profile_denial(extra).await;
            assert!(msg.contains("spec.posture: nonroot") && msg.contains(want), "{msg}");
        }
    }

    #[tokio::test]
    async fn nonroot_profiles_with_privileged_passthrough_containers_are_denied() {
        let cases = [
            (
                serde_json::json!({ "containers": [{
                    "name": "db",
                    "image": "registry.example.com/postgres:16",
                    "securityContext": { "privileged": true }
                }] }),
                "`db` is privileged",
            ),
            (
                serde_json::json!({ "initContainers": [{
                    "name": "migrate",
                    "image": "registry.example.com/m:1",
                    "securityContext": { "capabilities": { "add": ["NET_RAW"] } }
                }] }),
                "`migrate` adds capability `NET_RAW`",
            ),
            (
                serde_json::json!({ "containers": [{
                    "name": "db",
                    "image": "registry.example.com/postgres:16",
                    "securityContext": { "runAsUser": 0 }
                }] }),
                "`db` sets runAsUser: 0",
            ),
        ];
        for (extra, want) in cases {
            let msg = nonroot_profile_denial(extra).await;
            assert!(msg.contains("spec.posture: nonroot") && msg.contains(want), "{msg}");
        }
    }

    #[tokio::test]
    async fn root_profiles_keep_overlay_settings_and_privileged_sidecars() {
        let profile: WorkerProfileSpec = serde_json::from_value(serde_json::json!({
            "image": "registry.example.com/worker:latest",
            "posture": "root",
            "env": [{ "name": "WARM_REPO_DIR", "value": "/warm" }],
            "containers": [{
                "name": "dind",
                "image": "registry.example.com/dind:1",
                "securityContext": { "privileged": true, "runAsUser": 0 }
            }],
        }))
        .unwrap();
        assert!(check_posture(&profile).is_ok());
        let pod = dispatched_pod("p", &profile);
        assert!(matches!(decide(&pod, &source("p", profile)).await, Decision::Allow));
    }

    #[tokio::test]
    async fn pod_rendered_for_another_posture_is_denied() {
        let root_pod = dispatched_pod("lean", &lean_profile());
        let msg = deny_msg(decide(&root_pod, &source("lean", nonroot_profile())).await);
        assert!(msg.contains("rendered for posture `root`"), "{msg}");

        let nonroot_pod = dispatched_pod("lean", &nonroot_profile());
        let msg = deny_msg(decide(&nonroot_pod, &source("lean", lean_profile())).await);
        assert!(msg.contains("rendered for posture `nonroot`"), "{msg}");
    }

    #[tokio::test]
    async fn unknown_posture_annotation_is_denied() {
        let msg = deny_msg(
            decide_nonroot(|pod| {
                pod.metadata
                    .annotations
                    .as_mut()
                    .unwrap()
                    .insert(ANNOTATION_POSTURE.to_owned(), "restricted".to_owned());
            })
            .await,
        );
        assert!(msg.contains("names no posture"), "{msg}");
    }

    #[test]
    fn template_drift_reports_a_posture_change() {
        let pod = dispatched_pod("lean", &lean_profile());
        let template = PodTemplateSpec { metadata: Some(pod.metadata), spec: pod.spec };
        assert!(template_drift(&template, &lean_profile()).is_none());
        let drift = template_drift(&template, &nonroot_profile()).expect("posture drift");
        assert!(drift.contains("posture"), "{drift}");
    }

    #[tokio::test]
    async fn non_ref_literals_mentioning_aws_sm_are_allowed() {
        for value in ["aws-sm", "AWS-SM:upper", "region=aws-sm:x", "https://aws-sm.example"] {
            assert!(
                matches!(decide_with_worker_env(value).await, Decision::Allow),
                "{value:?} must be allowed"
            );
        }
    }
}
