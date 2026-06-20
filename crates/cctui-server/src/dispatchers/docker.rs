//! In-process Docker dispatcher (CCT-234).
//!
//! TRANSITIONAL — to be removed (CCT-248). The corrected model makes the server
//! an orchestrator that never touches the docker API: these container mechanics
//! move into a standalone, per-account *enrolled* `cctui-dispatcher-docker`
//! executor (CCT-246) that the server reaches over the wire, then this module
//! is deleted. Kept here only until that executor lands and soaks.
//!
//! Runs the same claude-worker image as a one-shot container via bollard against
//! a configured docker socket/host. Intended for the local self-host
//! docker-compose stack (CCT-217); the k8s server pod has NO docker socket, so
//! this dispatcher is only registered when `CCTUI_DISPATCHERS` declares a docker
//! instance AND a ping against its socket succeeds — otherwise it stays
//! unregistered (the caller logs and skips it).
//!
//! Env injection mirrors [`super::kube::KubeDispatcher`]: `SESSION_ID`,
//! `TASK_ID`, `TASK_PAYLOAD_JSON` (machine key stripped, CCT-191),
//! `CCTUI_MACHINE_KEY`, optional `TASK_NAME` / `REPLY_URL` / `CCTUI_URL`. The
//! container is labelled `cctui.dev/session-id=<sanitized>` for discovery and
//! created with `AutoRemove` so it cleans up on exit.
#![allow(clippy::doc_markdown)]

use std::collections::HashMap;

use async_trait::async_trait;
use bollard::Docker;
use bollard::container::{
    Config as ContainerConfig, CreateContainerOptions, StartContainerOptions,
};
use bollard::models::HostConfig;
use sha1::{Digest, Sha1};

use super::{DispatchError, DispatchHandle, DispatchSpec, Dispatcher, HandleStatus};

const LABEL_ORIGIN: &str = "cctui.dev/origin";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";

/// Config for one DockerDispatcher instance, parsed from `CCTUI_DISPATCHERS`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DockerDispatcherConfig {
    pub id: String,
    /// Worker image to run.
    pub image: String,
    /// Docker host/socket. Defaults to the standard local socket when unset.
    #[serde(default)]
    pub docker_host: Option<String>,
    /// Optional docker network to attach the container to.
    #[serde(default)]
    pub network: Option<String>,
    /// `CCTUI_URL` injected into the worker so its daemon dials back.
    #[serde(default)]
    pub cctui_url: Option<String>,
    /// Optional bind mounts (`/host:/container[:ro]`).
    #[serde(default)]
    pub mounts: Vec<String>,
}

pub struct DockerDispatcher {
    id: String,
    image: String,
    network: Option<String>,
    cctui_url: Option<String>,
    mounts: Vec<String>,
    docker: Docker,
}

impl DockerDispatcher {
    /// Connect to the configured docker host (or the local default) and ping it.
    /// Returns `Err` when no docker is reachable so the caller leaves the
    /// dispatcher unregistered (the server pod has no socket).
    pub async fn try_new(cfg: &DockerDispatcherConfig) -> anyhow::Result<Self> {
        let docker = match &cfg.docker_host {
            Some(host) => Docker::connect_with_http(host, 60, bollard::API_DEFAULT_VERSION)?,
            None => Docker::connect_with_local_defaults()?,
        };
        docker.ping().await?;
        Ok(Self {
            id: cfg.id.clone(),
            image: cfg.image.clone(),
            network: cfg.network.clone(),
            cctui_url: cfg.cctui_url.clone(),
            mounts: cfg.mounts.clone(),
            docker,
        })
    }

    fn container_name(session_id: &str) -> String {
        let digest = Sha1::digest(session_id.as_bytes());
        format!("cctui-worker-{}", &hex::encode(digest)[..12])
    }

    fn label_safe(value: &str) -> String {
        super::kube::KubeDispatcher::label_safe_for(value)
    }

    fn build_env(&self, spec: &DispatchSpec<'_>) -> Result<Vec<String>, DispatchError> {
        let mut payload = spec.payload.clone();
        let machine_key = payload
            .as_object_mut()
            .and_then(|o| o.remove("cctui_machine_key"))
            .and_then(|v| v.as_str().map(ToOwned::to_owned));
        let task_name = payload.get("name").and_then(|v| v.as_str()).map(ToOwned::to_owned);
        let payload_json = serde_json::to_string(&payload)
            .map_err(|e| DispatchError::Backend(format!("serializing payload: {e}")))?;

        let mut env = vec![
            format!("SESSION_ID={}", spec.session_id),
            format!("TASK_ID={}", spec.session_id),
            format!("TASK_PAYLOAD_JSON={payload_json}"),
        ];
        if let Some(url) = &self.cctui_url {
            env.push(format!("CCTUI_URL={url}"));
        }
        if let Some(n) = task_name {
            env.push(format!("TASK_NAME={n}"));
        }
        if let Some(k) = machine_key {
            env.push(format!("CCTUI_MACHINE_KEY={k}"));
        }
        if let Some(u) = spec.reply_url {
            env.push(format!("REPLY_URL={u}"));
        }
        Ok(env)
    }
}

#[async_trait]
impl Dispatcher for DockerDispatcher {
    fn id(&self) -> &str {
        &self.id
    }

    async fn dispatch(&self, spec: &DispatchSpec<'_>) -> Result<DispatchHandle, DispatchError> {
        if spec.session_id.is_empty() {
            return Err(DispatchError::InvalidIntent("session_id is required".into()));
        }
        let name = Self::container_name(spec.session_id);
        let env = self.build_env(spec)?;

        let mut labels = HashMap::new();
        labels.insert(LABEL_ORIGIN.to_owned(), "cctui-docker-dispatcher".to_owned());
        labels.insert(LABEL_SESSION_ID.to_owned(), Self::label_safe(spec.session_id));

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

        // Idempotency: a repeat dispatch of the same session reuses the
        // deterministic container name; a 409 (name in use) means it already
        // exists, so dedup rather than clobbering.
        let opts = CreateContainerOptions { name: name.clone(), platform: None };
        match self.docker.create_container(Some(opts), config).await {
            Ok(_) => {}
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 409, .. }) => {
                return Ok(DispatchHandle {
                    handle: format!("container/{name}"),
                    namespace: None,
                    status: Some("deduplicated".into()),
                });
            }
            Err(e) => return Err(DispatchError::Backend(format!("creating container: {e}"))),
        }

        self.docker
            .start_container(&name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| DispatchError::Backend(format!("starting container: {e}")))?;

        Ok(DispatchHandle {
            handle: format!("container/{name}"),
            namespace: None,
            status: Some("dispatched".into()),
        })
    }

    async fn status(&self, handle: &str) -> Result<HandleStatus, DispatchError> {
        let name = handle.strip_prefix("container/").unwrap_or(handle);
        match self.docker.inspect_container(name, None).await {
            Ok(c) => {
                let state = c.state.as_ref();
                let running = state.and_then(|s| s.running).unwrap_or(false);
                if running {
                    return Ok(HandleStatus::Running);
                }
                let exit = state.and_then(|s| s.exit_code).unwrap_or(0);
                Ok(if exit == 0 {
                    HandleStatus::Complete
                } else {
                    HandleStatus::Failed(Some(format!("container exited with code {exit}")))
                })
            }
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                Ok(HandleStatus::Gone)
            }
            Err(e) => Err(DispatchError::Backend(format!("inspecting container {name}: {e}"))),
        }
    }

    async fn cancel(&self, handle: &str) -> Result<(), DispatchError> {
        let name = handle.strip_prefix("container/").unwrap_or(handle);
        let opts = bollard::container::RemoveContainerOptions { force: true, ..Default::default() };
        match self.docker.remove_container(name, Some(opts)).await {
            // 404 = already gone (auto-remove); treat as a successful cancel.
            Ok(())
            | Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                Ok(())
            }
            Err(e) => Err(DispatchError::Backend(format!("removing container {name}: {e}"))),
        }
    }
}
