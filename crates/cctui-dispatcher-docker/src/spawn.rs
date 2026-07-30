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
        let base = build_env(spec, &self.cctui_url)?;
        let mut env = base.env;
        if let Some(k) = base.machine_key {
            env.push(format!("CCTUI_MACHINE_KEY={k}"));
        }
        Ok(env)
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

        let mut labels = HashMap::new();
        labels.insert(LABEL_ORIGIN.to_owned(), "cctui-docker-dispatcher".to_owned());
        labels.insert(LABEL_SESSION_ID.to_owned(), label_safe(&spec.session_id));

        let host_config = HostConfig {
            auto_remove: Some(true),
            binds: if self.mounts.is_empty() { None } else { Some(self.mounts.clone()) },
            network_mode: self.network.clone(),
            ..Default::default()
        };

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
