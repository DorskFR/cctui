//! How a failed worker's container ended and what it last wrote, read while
//! the finished Job still owns its pods.

use cctui_orchestrator::{ANNOTATION_WORKER_CONTAINER, DEFAULT_WORKER_CONTAINER};
use cctui_proto::api::WorkerExit;
use k8s_openapi::api::core::v1::{ContainerStateTerminated, ContainerStatus, Pod};
use kube::Error as KubeError;
use kube::api::{ListParams, LogParams};

use super::Spawner;

const LOG_TAIL_LINES: usize = 40;
const LOG_TAIL_BYTES: i64 = 16 * 1024;

/// The pod and container whose logs explain the exit.
#[derive(Debug, PartialEq, Eq)]
struct LogTarget {
    pod: String,
    container: String,
    /// The container restarted: the logs that matter are the previous run's.
    previous: bool,
}

fn worker_container(pod: &Pod) -> &str {
    pod.metadata
        .annotations
        .as_ref()
        .and_then(|a| a.get(ANNOTATION_WORKER_CONTAINER))
        .map_or(DEFAULT_WORKER_CONTAINER, String::as_str)
}

fn termination(cs: &ContainerStatus) -> Option<(&ContainerStateTerminated, bool)> {
    let current = cs.state.as_ref().and_then(|s| s.terminated.as_ref()).map(|t| (t, false));
    current.or_else(|| cs.last_state.as_ref()?.terminated.as_ref().map(|t| (t, true)))
}

/// The newest of the Job's pods, how its worker container ended, and where to
/// read its logs.
///
/// A pod fails on any container, so when the worker exited cleanly a container
/// that did not is reported instead.
fn exit_of(pods: &[Pod]) -> Option<(WorkerExit, LogTarget)> {
    let pod = pods.iter().max_by_key(|p| p.metadata.creation_timestamp.as_ref().map(|t| t.0))?;
    let pod_name = pod.metadata.name.clone()?;
    let worker = worker_container(pod);
    let statuses =
        pod.status.as_ref().and_then(|s| s.container_statuses.as_deref()).unwrap_or_default();
    let worker_end = statuses.iter().find(|cs| cs.name == worker).and_then(termination);
    let culprit = if worker_end.is_some_and(|(t, _)| t.exit_code == 0) {
        statuses.iter().find_map(|cs| {
            termination(cs).filter(|(t, _)| t.exit_code != 0).map(|end| (cs.name.as_str(), end))
        })
    } else {
        None
    };
    let (container, end) = culprit.map_or((worker, worker_end), |(name, end)| (name, Some(end)));
    let (terminated, previous) = end.map_or((None, false), |(t, previous)| (Some(t), previous));
    let exit = WorkerExit {
        container: Some(container.to_owned()),
        exit_code: terminated.map(|t| t.exit_code),
        reason: terminated.and_then(|t| t.reason.clone().or_else(|| t.message.clone())),
        signal: terminated.and_then(|t| t.signal),
        log_tail: None,
        logs_error: None,
    };
    Some((exit, LogTarget { pod: pod_name, container: container.to_owned(), previous }))
}

/// The last `lines` lines, without a trailing newline.
fn tail(text: &str, lines: usize) -> Option<String> {
    let text = text.trim_end();
    if text.is_empty() {
        return None;
    }
    let skip = text.lines().count().saturating_sub(lines);
    Some(text.lines().skip(skip).collect::<Vec<_>>().join("\n"))
}

/// Why the logs could not be read, naming the grant a 403 lacks.
fn logs_error(code: u16, message: &str, namespace: &str) -> String {
    match code {
        403 => format!(
            "the dispatcher may not read pod logs: its ServiceAccount needs `get` on \
             `pods/log` (core API group) in namespace `{namespace}`"
        ),
        404 => "the pod is gone, and its logs with it".to_owned(),
        _ => format!("pod logs unreadable ({code}): {message}"),
    }
}

impl Spawner {
    pub(super) async fn read_worker_exit(&self, handle: &str) -> Option<WorkerExit> {
        let name = handle.strip_prefix("jobs/").unwrap_or(handle);
        let lp = ListParams::default().labels(&format!("job-name={name}"));
        let pods = self.pods().list(&lp).await.ok()?.items;
        let (mut exit, target) = exit_of(&pods)?;
        let params = LogParams {
            container: Some(target.container),
            previous: target.previous,
            tail_lines: i64::try_from(LOG_TAIL_LINES).ok(),
            limit_bytes: Some(LOG_TAIL_BYTES),
            ..LogParams::default()
        };
        match self.pods().logs(&target.pod, &params).await {
            Ok(text) => exit.log_tail = tail(&text, LOG_TAIL_LINES),
            Err(KubeError::Api(status)) => {
                exit.logs_error = Some(logs_error(status.code, &status.message, &self.namespace));
            }
            Err(err) => exit.logs_error = Some(format!("pod logs unreadable: {err}")),
        }
        Some(exit)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn pod(name: &str, created: &str, status: &serde_json::Value) -> Pod {
        serde_json::from_value(json!({
            "metadata": {
                "name": name,
                "creationTimestamp": created,
                "annotations": { ANNOTATION_WORKER_CONTAINER: "agent" }
            },
            "status": status
        }))
        .unwrap()
    }

    #[test]
    fn the_newest_pod_reports_the_worker_containers_exit() {
        let older = pod(
            "w-old",
            "2026-10-21T08:00:00Z",
            &json!({ "containerStatuses": [{
                "name": "agent", "image": "i", "imageID": "", "ready": false, "restartCount": 0,
                "state": { "terminated": { "exitCode": 2, "reason": "Error" } }
            }]}),
        );
        let newer = pod(
            "w-new",
            "2026-10-21T08:05:00Z",
            &json!({ "phase": "Failed", "containerStatuses": [
                {
                    "name": "guard-proxy", "image": "i", "imageID": "", "ready": false,
                    "restartCount": 0, "state": { "terminated": { "exitCode": 0 } }
                },
                {
                    "name": "agent", "image": "i", "imageID": "", "ready": false,
                    "restartCount": 0,
                    "state": { "terminated": {
                        "exitCode": 137, "reason": "Error", "signal": 9
                    } }
                }
            ]}),
        );
        let (exit, target) = exit_of(&[older, newer]).unwrap();
        assert_eq!(
            exit,
            WorkerExit {
                container: Some("agent".into()),
                exit_code: Some(137),
                reason: Some("Error".into()),
                signal: Some(9),
                log_tail: None,
                logs_error: None,
            }
        );
        assert_eq!(
            target,
            LogTarget { pod: "w-new".into(), container: "agent".into(), previous: false }
        );
    }

    #[test]
    fn a_sidecar_that_failed_the_pod_is_reported_when_the_worker_exited_cleanly() {
        let p = pod(
            "w",
            "2026-10-21T05:00:00Z",
            &json!({ "phase": "Failed", "containerStatuses": [
                {
                    "name": "agent", "image": "i", "imageID": "", "ready": false,
                    "restartCount": 0, "state": { "terminated": { "exitCode": 0 } }
                },
                {
                    "name": "guard-proxy", "image": "i", "imageID": "", "ready": false,
                    "restartCount": 0,
                    "state": { "terminated": { "exitCode": 143, "reason": "Error" } }
                }
            ]}),
        );
        let (exit, target) = exit_of(&[p]).unwrap();
        assert_eq!(exit.container.as_deref(), Some("guard-proxy"));
        assert_eq!((exit.exit_code, exit.reason.as_deref()), (Some(143), Some("Error")));
        assert_eq!(target.container, "guard-proxy");
    }

    #[test]
    fn a_restarted_container_reads_its_previous_run() {
        let p = pod(
            "w",
            "2026-10-21T08:00:00Z",
            &json!({ "containerStatuses": [{
                "name": "agent", "image": "i", "imageID": "", "ready": false, "restartCount": 3,
                "state": { "waiting": { "reason": "CrashLoopBackOff" } },
                "lastState": { "terminated": { "exitCode": 1, "message": "panic: boom" } }
            }]}),
        );
        let (exit, target) = exit_of(&[p]).unwrap();
        assert_eq!(exit.exit_code, Some(1));
        assert_eq!(exit.reason.as_deref(), Some("panic: boom"));
        assert!(target.previous);
    }

    #[test]
    fn a_pod_without_annotations_uses_the_default_container_and_no_pod_is_none() {
        let p: Pod = serde_json::from_value(json!({
            "metadata": { "name": "w", "creationTimestamp": "2026-10-21T08:00:00Z" }
        }))
        .unwrap();
        let (exit, target) = exit_of(&[p]).unwrap();
        assert_eq!(target.container, DEFAULT_WORKER_CONTAINER);
        assert_eq!(exit.exit_code, None);
        assert!(exit_of(&[]).is_none());
    }

    #[test]
    fn the_log_tail_keeps_the_last_lines() {
        let log = (1..=100).map(|i| format!("line {i}\n")).collect::<Vec<_>>().concat();
        let kept = tail(&log, LOG_TAIL_LINES).unwrap();
        assert_eq!(kept.lines().count(), LOG_TAIL_LINES);
        assert!(kept.starts_with("line 61\n") && kept.ends_with("line 100"));
        assert_eq!(tail("a\nb\n", LOG_TAIL_LINES).as_deref(), Some("a\nb"));
        assert_eq!(tail("\n", LOG_TAIL_LINES), None);
    }

    #[test]
    fn a_forbidden_log_read_names_the_missing_grant() {
        let status: kube::core::Status = serde_json::from_value(json!({
            "kind": "Status", "apiVersion": "v1", "status": "Failure", "code": 403,
            "reason": "Forbidden",
            "message": "pods \"w\" is forbidden: User \"system:serviceaccount:cctui:cctui-dispatcher-kube\" cannot get resource \"pods/log\""
        }))
        .unwrap();
        let err = logs_error(status.code, &status.message, "cctui");
        assert!(err.contains("`get` on `pods/log`"), "{err}");
        assert!(err.contains("`cctui`"), "{err}");
        assert_eq!(logs_error(404, "", "cctui"), "the pod is gone, and its logs with it");
        assert!(logs_error(500, "etcd down", "cctui").contains("etcd down"));
    }
}
