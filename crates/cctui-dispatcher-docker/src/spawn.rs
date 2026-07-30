//! Docker spawn mechanics for the standalone docker dispatcher.
//!
//! bollard against the local socket, deterministic
//! `cctui-worker-<sha1(dedup)[:12]>` naming for idempotency, env injection with
//! `cctui_machine_key` lifted out of the payload, `AutoRemove`, and discovery
//! labels. The shared WS/enroll plumbing lives in `cctui-dispatcher-core`; this
//! is the platform-specific `HostConfig` builder behind [`Dispatcher`].
//!
//! ⚠️ Repo is PUBLIC — no homelab-specific images/hosts/networks here; the
//! image + host come from the dispatcher's own config.
#![allow(clippy::doc_markdown)]

use std::collections::HashMap;

use bollard::Docker;
use bollard::container::{
    Config as ContainerConfig, CreateContainerOptions, StartContainerOptions,
};
use bollard::models::HostConfig;
use cctui_dispatcher_core::{
    Dispatcher, HandleState, SpawnOutcome, build_env, dedup_source, label_safe, worker_name,
};
use cctui_proto::ws::WireDispatchSpec;

const LABEL_ORIGIN: &str = "cctui.dev/origin";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";

pub struct Spawner {
    image: String,
    network: Option<String>,
    cctui_url: String,
    mounts: Vec<String>,
    docker: Docker,
}

impl Spawner {
    /// Connect to the configured docker host (or the local default) and ping it
    /// so a missing socket fails loudly at startup rather than on first
    /// dispatch.
    pub async fn connect(
        docker_host: Option<&str>,
        image: String,
        network: Option<String>,
        cctui_url: String,
        mounts: Vec<String>,
    ) -> anyhow::Result<Self> {
        let docker = match docker_host {
            Some(host) => Docker::connect_with_http(host, 60, bollard::API_DEFAULT_VERSION)?,
            None => Docker::connect_with_local_defaults()?,
        };
        docker.ping().await?;
        Ok(Self { image, network, cctui_url, mounts, docker })
    }

    fn worker_env(&self, spec: &WireDispatchSpec) -> anyhow::Result<Vec<String>> {
        Self::build_worker_env(spec, &self.cctui_url)
    }

    fn build_worker_env(spec: &WireDispatchSpec, cctui_url: &str) -> anyhow::Result<Vec<String>> {
        let base = build_env(spec, cctui_url)?;
        let mut env = base.env;
        if let Some(k) = base.machine_key {
            env.push(format!("CCTUI_MACHINE_KEY={k}"));
        }
        Ok(env)
    }

    fn discovery_labels(session_id: &str) -> HashMap<String, String> {
        let mut labels = HashMap::new();
        labels.insert(LABEL_ORIGIN.to_owned(), "cctui-docker-dispatcher".to_owned());
        labels.insert(LABEL_SESSION_ID.to_owned(), label_safe(session_id));
        labels
    }

    fn host_config(mounts: &[String], network: Option<&str>) -> HostConfig {
        HostConfig {
            auto_remove: Some(true),
            binds: if mounts.is_empty() { None } else { Some(mounts.to_vec()) },
            network_mode: network.map(ToOwned::to_owned),
            ..Default::default()
        }
    }
}

impl Dispatcher for Spawner {
    fn kind(&self) -> &'static str {
        "docker"
    }

    /// Spawn a worker container for the session. Idempotent: a repeat dispatch
    /// of the same key reuses the deterministic name; a 409 (name in use) is
    /// reported as `deduplicated` rather than clobbering the running worker.
    async fn dispatch(&self, spec: &WireDispatchSpec) -> anyhow::Result<SpawnOutcome> {
        if spec.session_id.is_empty() {
            anyhow::bail!("session_id is required");
        }
        let name = worker_name(dedup_source(spec));
        let env = self.worker_env(spec)?;

        let labels = Self::discovery_labels(&spec.session_id);
        let host_config = Self::host_config(&self.mounts, self.network.as_deref());

        let config = ContainerConfig {
            image: Some(self.image.clone()),
            env: Some(env),
            labels: Some(labels),
            host_config: Some(host_config),
            ..Default::default()
        };

        let opts = CreateContainerOptions { name: name.clone(), platform: None };
        match self.docker.create_container(Some(opts), config).await {
            Ok(_) => {}
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 409, .. }) => {
                return Ok(SpawnOutcome {
                    handle: format!("container/{name}"),
                    status: "deduplicated".to_owned(),
                    namespace: None,
                });
            }
            Err(e) => anyhow::bail!("creating container: {e}"),
        }

        self.docker
            .start_container(&name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| anyhow::anyhow!("starting container: {e}"))?;

        Ok(SpawnOutcome {
            handle: format!("container/{name}"),
            status: "dispatched".to_owned(),
            namespace: None,
        })
    }

    /// Lifecycle of a container handle, plus a human reason when it FAILED — a
    /// non-zero exit, or a wedged restart loop. The server lifts the reason into
    /// the completion webhook's `error`.
    async fn status(&self, handle: &str) -> anyhow::Result<(HandleState, Option<String>)> {
        let name = handle.strip_prefix("container/").unwrap_or(handle);
        match self.docker.inspect_container(name, None).await {
            Ok(c) => {
                let state = c.state.as_ref();
                let running = state.and_then(|s| s.running).unwrap_or(false);
                // A container Docker is restarting (restart policy looping on a
                // crashing process) is doomed, not live.
                let restarting = state.and_then(|s| s.restarting).unwrap_or(false);
                if restarting {
                    return Ok((HandleState::Failed, Some("container restart-looping".to_owned())));
                }
                if running {
                    return Ok((HandleState::Running, None));
                }
                let oom = state.and_then(|s| s.oom_killed).unwrap_or(false);
                let exit = state.and_then(|s| s.exit_code).unwrap_or(0);
                if oom {
                    return Ok((HandleState::Failed, Some("OOMKilled".to_owned())));
                }
                Ok(if exit == 0 {
                    (HandleState::Complete, None)
                } else {
                    (HandleState::Failed, Some(format!("container exited with code {exit}")))
                })
            }
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                Ok((HandleState::Gone, None))
            }
            Err(e) => anyhow::bail!("inspecting container {name}: {e}"),
        }
    }

    async fn cancel(&self, handle: &str) -> anyhow::Result<()> {
        let name = handle.strip_prefix("container/").unwrap_or(handle);
        let opts = bollard::container::RemoveContainerOptions { force: true, ..Default::default() };
        match self.docker.remove_container(name, Some(opts)).await {
            // 404 = already gone (auto-remove); treat as a successful cancel.
            Ok(())
            | Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                Ok(())
            }
            Err(e) => anyhow::bail!("removing container {name}: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn spec(session_id: &str, payload: serde_json::Value) -> WireDispatchSpec {
        WireDispatchSpec {
            session_id: session_id.to_owned(),
            timeout_minutes: Some(30),
            reply_url: Some("https://cb.example.test".to_owned()),
            dedup_key: None,
            profile: None,
            payload,
        }
    }

    #[test]
    fn host_config_is_unprivileged_and_auto_removes() {
        let hc = Spawner::host_config(&[], None);
        assert_eq!(hc.privileged, None, "must never request a privileged container");
        assert_eq!(hc.auto_remove, Some(true));
        assert_eq!(hc.binds, None, "no mounts => no binds");
        assert_eq!(hc.network_mode, None, "no host network / no explicit network");
        assert_eq!(hc.cap_add, None, "no added capabilities");
        assert_eq!(hc.pid_mode, None, "no host PID namespace");
        assert_eq!(hc.ipc_mode, None, "no host IPC namespace");
        assert_eq!(hc.userns_mode, None, "no host user namespace");
    }

    #[test]
    fn host_config_preserves_read_only_binds() {
        let mounts =
            vec!["/host/cache:/cache:ro".to_owned(), "/host/certs:/etc/ssl/certs:ro".to_owned()];
        let hc = Spawner::host_config(&mounts, Some("cctui-net"));
        assert_eq!(hc.privileged, None);
        let binds = hc.binds.expect("binds present");
        assert_eq!(binds, mounts, "configured RO binds pass through verbatim");
        assert!(binds.iter().all(|b| b.ends_with(":ro")), "the configured binds are read-only");
        assert_eq!(
            hc.network_mode.as_deref(),
            Some("cctui-net"),
            "explicit named network, not host"
        );
        assert_ne!(hc.network_mode.as_deref(), Some("host"));
    }

    #[test]
    fn discovery_labels_stamp_origin_and_sanitized_session() {
        let labels = Spawner::discovery_labels("triage:PROJ:2026");
        assert_eq!(labels.get(LABEL_ORIGIN).map(String::as_str), Some("cctui-docker-dispatcher"));
        assert_eq!(labels.get(LABEL_SESSION_ID).map(String::as_str), Some("triage-PROJ-2026"));
    }

    #[test]
    fn worker_env_injects_contract_and_lifts_machine_key_out_of_payload() {
        let s = spec(
            "sess-123",
            json!({ "name": "Review #7", "cctui_machine_key": "SECRET", "flow": "review" }),
        );
        let env = Spawner::build_worker_env(&s, "https://cctui.example.test").unwrap();
        assert!(env.contains(&"SESSION_ID=sess-123".to_owned()));
        assert!(env.contains(&"TASK_ID=sess-123".to_owned()));
        assert!(env.contains(&"TASK_NAME=Review #7".to_owned()));
        assert!(env.contains(&"CCTUI_URL=https://cctui.example.test".to_owned()));
        assert!(env.contains(&"REPLY_URL=https://cb.example.test".to_owned()));
        assert!(env.contains(&"CCTUI_MACHINE_KEY=SECRET".to_owned()));
        let tp = env.iter().find(|e| e.starts_with("TASK_PAYLOAD_JSON=")).unwrap();
        assert!(!tp.contains("SECRET"), "machine key leaked into payload: {tp}");
        assert!(tp.contains("review"));
    }

    #[test]
    fn worker_env_omits_machine_key_when_payload_has_none() {
        let s = spec("sess-9", json!({ "flow": "review" }));
        let env = Spawner::build_worker_env(&s, "https://cctui.example.test").unwrap();
        assert!(env.iter().all(|e| !e.starts_with("CCTUI_MACHINE_KEY=")));
    }
}
