//! The guard Pod and Service of a `pod`-placed dispatch, derived from its Job.

use cctui_orchestrator::envelope::{GuardPod, default_sidecar_image, guard_pod, guard_service};
use cctui_orchestrator::{
    ANNOTATION_DISPATCH_ID, ANNOTATION_GUARD_CONTROL_TOKEN, ANNOTATION_GUARD_IDENTITY,
    ANNOTATION_GUARD_PROXY_TOKEN, ANNOTATION_GUARD_SERVICE, ANNOTATION_SERVER_URL, GuardPlacement,
    LABEL_GUARD_PLACEMENT, WorkerProfileSpec,
};
use k8s_openapi::api::batch::v1::Job;
use k8s_openapi::api::core::v1::{Pod, Service};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::OwnerReference;
use serde_json::{Map, Value, json};

/// Bounds a guard left behind while the dispatcher is down.
const DEADLINE_MARGIN_SECS: i64 = 300;

const GUARD_ANNOTATIONS: [&str; 3] =
    [ANNOTATION_GUARD_SERVICE, ANNOTATION_GUARD_PROXY_TOKEN, ANNOTATION_GUARD_CONTROL_TOKEN];

/// `<job>-g<4 hex>`: one name per dispatch, so a redispatch never meets the
/// previous run's guard, and the `<job>-<5 lowercase chars>` the server
/// redeems for.
pub fn name(job: &str, dispatch_id: &str) -> String {
    let hex: String = dispatch_id
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|c| c.to_ascii_lowercase())
        .chain(std::iter::repeat('0'))
        .take(4)
        .collect();
    format!("{job}-g{hex}")
}

fn token() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

pub fn is_pod(profile: &WorkerProfileSpec) -> bool {
    profile.guard_placement() == GuardPlacement::Pod
}

pub fn sidecar_image() -> String {
    std::env::var("CCTUI_ORCH_SIDECAR_IMAGE")
        .ok()
        .map(|v| v.trim().to_owned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(default_sidecar_image)
}

pub fn stamp(
    annotations: &mut Map<String, Value>,
    labels: &mut Map<String, Value>,
    profile: &WorkerProfileSpec,
    job: &str,
    dispatch_id: &str,
) {
    if !is_pod(profile) {
        for key in GUARD_ANNOTATIONS {
            annotations.remove(key);
        }
        labels.remove(LABEL_GUARD_PLACEMENT);
        return;
    }
    annotations
        .entry(ANNOTATION_GUARD_SERVICE)
        .or_insert_with(|| json!(name(job, dispatch_id)));
    for key in [ANNOTATION_GUARD_PROXY_TOKEN, ANNOTATION_GUARD_CONTROL_TOKEN] {
        annotations.entry(key).or_insert_with(|| json!(token()));
    }
    labels.insert(LABEL_GUARD_PLACEMENT.into(), json!("pod"));
}

pub fn manifests(job: &Job, profile: &WorkerProfileSpec, image: &str) -> Option<(Pod, Service)> {
    let job_name = job.metadata.name.as_deref()?;
    let uid = job.metadata.uid.as_deref()?;
    let spec = job.spec.as_ref()?;
    let template = spec.template.metadata.as_ref()?.annotations.as_ref()?;
    let mut annotations = std::collections::BTreeMap::new();
    let name = template.get(ANNOTATION_GUARD_SERVICE)?.clone();
    let required =
        [ANNOTATION_DISPATCH_ID, ANNOTATION_GUARD_PROXY_TOKEN, ANNOTATION_GUARD_CONTROL_TOKEN];
    for key in required {
        annotations.insert(key.to_owned(), template.get(key)?.clone());
    }
    for key in [ANNOTATION_SERVER_URL, ANNOTATION_GUARD_IDENTITY] {
        if let Some(v) = template.get(key) {
            annotations.insert(key.to_owned(), v.clone());
        }
    }
    let owner = OwnerReference {
        api_version: "batch/v1".to_owned(),
        kind: "Job".to_owned(),
        name: job_name.to_owned(),
        uid: uid.to_owned(),
        ..OwnerReference::default()
    };
    let pod = guard_pod(&GuardPod {
        name: &name,
        job: job_name,
        sidecar_image: image,
        profile,
        annotations,
        owner: Some(owner.clone()),
        active_deadline_seconds: spec.active_deadline_seconds.map(|d| d + DEADLINE_MARGIN_SECS),
    });
    Some((pod, guard_service(&name, job_name, Some(owner))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(placement: &str) -> WorkerProfileSpec {
        serde_json::from_value(json!({
            "image": "example.com/worker:latest",
            "posture": "nonroot",
            "guard": { "placement": placement },
        }))
        .unwrap()
    }

    #[test]
    fn the_guard_pod_name_is_per_dispatch_and_passes_the_servers_job_pod_check() {
        let job = "cctui-worker-0123456789ab";
        let a = name(job, "0D157A7C-4000-4000-8000-000000000001");
        assert_eq!(a, format!("{job}-g0d15"));
        let suffix = a.strip_prefix(&format!("{job}-")).unwrap().to_owned();
        assert_eq!(suffix.len(), 5);
        assert!(suffix.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()));
        assert_ne!(a, name(job, "9f3e0000-0000-4000-8000-000000000000"));
        assert_eq!(name(job, ""), format!("{job}-g0000"));
    }

    #[test]
    fn stamping_mints_distinct_tokens_once_and_a_sidecar_strips_them() {
        let (mut annotations, mut labels) = (Map::new(), Map::new());
        stamp(&mut annotations, &mut labels, &profile("pod"), "job-a", "ab12-cd");
        let proxy = annotations[ANNOTATION_GUARD_PROXY_TOKEN].as_str().unwrap().to_owned();
        let control = annotations[ANNOTATION_GUARD_CONTROL_TOKEN].as_str().unwrap().to_owned();
        assert_eq!(proxy.len(), 64);
        assert_ne!(proxy, control);
        assert_eq!(annotations[ANNOTATION_GUARD_SERVICE], "job-a-gab12");
        assert_eq!(labels[LABEL_GUARD_PLACEMENT], "pod");

        stamp(&mut annotations, &mut labels, &profile("pod"), "job-a", "ffff");
        assert_eq!(annotations[ANNOTATION_GUARD_SERVICE], "job-a-gab12");
        assert_eq!(annotations[ANNOTATION_GUARD_PROXY_TOKEN], proxy.as_str());

        stamp(&mut annotations, &mut labels, &profile("sidecar"), "job-a", "ab12");
        assert!(annotations.is_empty() && labels.is_empty());
    }

    #[test]
    fn a_job_without_a_uid_or_tokens_yields_no_guard() {
        let job: Job = serde_json::from_value(json!({
            "metadata": { "name": "job-a" },
            "spec": { "template": { "metadata": { "annotations": {} } } }
        }))
        .unwrap();
        assert!(manifests(&job, &profile("pod"), "img").is_none());
    }
}
