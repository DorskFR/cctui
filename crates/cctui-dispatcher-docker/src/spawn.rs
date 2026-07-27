//! Docker spawn mechanics for the standalone docker dispatcher.
//!
//! Lifted from the transitional in-process `cctui-server/src/dispatchers/docker.rs`
//! (248): bollard against the local socket, deterministic
//! `cctui-worker-<sha1(session)[:12]>` naming for idempotency, env injection
//! with `cctui_machine_key` lifted out of the payload, `AutoRemove`,
//! and discovery labels. The server keeps its in-process copy as a transitional
//! shape until parts 2-4 land; this is the enrolled-executor home for
//! the same mechanics.
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
use cctui_proto::ws::WireDispatchSpec;
use sha1::{Digest, Sha1};

const LABEL_ORIGIN: &str = "cctui.dev/origin";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";

/// Lifecycle state of a spawned container handle.
#[derive(Debug, Clone, Copy)]
pub enum HandleState {
    Running,
    Complete,
    Failed,
    Gone,
}

impl HandleState {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Gone => "gone",
        }
    }
}

/// Outcome of a dispatch: an opaque handle plus the idempotency status reported
/// back to the server verbatim.
#[derive(Debug, Clone)]
pub struct SpawnOutcome {
    pub handle: String,
    pub status: String,
}

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

    fn container_name(dedup_source: &str) -> String {
        let digest = Sha1::digest(dedup_source.as_bytes());
        format!("cctui-worker-{}", &hex::encode(digest)[..12])
    }

    /// The string the container name derives from: the caller's `dedup_key` when
    /// present, else the `session_id`. Mirrors the kube dispatcher so
    /// `session_id` can be fresh per dispatch while a repeat of the same logical
    /// key still coalesces onto one container.
    fn dedup_source(spec: &WireDispatchSpec) -> &str {
        spec.dedup_key.as_deref().filter(|k| !k.is_empty()).unwrap_or(&spec.session_id)
    }

    /// Kubernetes/docker-safe label value: lowercase alnum / `-` / `_` / `.`,
    /// trimmed to 63 chars, with a stable fallback when empty.
    fn label_safe(value: &str) -> String {
        let mut out: String = value
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .take(63)
            .collect();
        let trimmed = out.trim_matches(|c| c == '-' || c == '.');
        if trimmed.is_empty() {
            "session".clone_into(&mut out);
        } else if trimmed.len() != out.len() {
            out = trimmed.to_owned();
        }
        out
    }

    fn build_env(&self, spec: &WireDispatchSpec) -> anyhow::Result<Vec<String>> {
        let mut payload = spec.payload.clone();
        let machine_key = payload
            .as_object_mut()
            .and_then(|o| o.remove("cctui_machine_key"))
            .and_then(|v| v.as_str().map(ToOwned::to_owned));
        let task_name = payload.get("name").and_then(|v| v.as_str()).map(ToOwned::to_owned);
        let payload_json = serde_json::to_string(&payload)?;

        let mut env = vec![
            format!("SESSION_ID={}", spec.session_id),
            format!("TASK_ID={}", spec.session_id),
            format!("TASK_PAYLOAD_JSON={payload_json}"),
            format!("CCTUI_URL={}", self.cctui_url),
        ];
        if let Some(n) = task_name {
            env.push(format!("TASK_NAME={n}"));
        }
        if let Some(k) = machine_key {
            env.push(format!("CCTUI_MACHINE_KEY={k}"));
        }
        if let Some(u) = &spec.reply_url {
            env.push(format!("REPLY_URL={u}"));
        }
        Ok(env)
    }

    /// Spawn a worker container for the session. Idempotent: a repeat dispatch
    /// of the same session reuses the deterministic name; a 409 (name in use)
    /// is reported as `deduplicated` rather than clobbering the running worker.
    pub async fn dispatch(&self, spec: &WireDispatchSpec) -> anyhow::Result<SpawnOutcome> {
        if spec.session_id.is_empty() {
            anyhow::bail!("session_id is required");
        }
        let name = Self::container_name(Self::dedup_source(spec));
        let env = self.build_env(spec)?;

        let mut labels = HashMap::new();
        labels.insert(LABEL_ORIGIN.to_owned(), "cctui-docker-dispatcher".to_owned());
        labels.insert(LABEL_SESSION_ID.to_owned(), Self::label_safe(&spec.session_id));

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
                });
            }
            Err(e) => anyhow::bail!("creating container: {e}"),
        }

        self.docker
            .start_container(&name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| anyhow::anyhow!("starting container: {e}"))?;

        Ok(SpawnOutcome { handle: format!("container/{name}"), status: "dispatched".to_owned() })
    }

    /// Lifecycle of a container handle, plus a human reason when it FAILED — a
    /// non-zero exit, or a wedged restart loop. The server lifts the
    /// reason into the completion webhook's `error`.
    pub async fn status(&self, handle: &str) -> anyhow::Result<(HandleState, Option<String>)> {
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

    pub async fn cancel(&self, handle: &str) -> anyhow::Result<()> {
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
    use super::*;

    #[test]
    fn container_name_is_deterministic_and_prefixed() {
        let a = Spawner::container_name("session-xyz");
        let b = Spawner::container_name("session-xyz");
        assert_eq!(a, b);
        assert!(a.starts_with("cctui-worker-"));
        assert_eq!(a.len(), "cctui-worker-".len() + 12);
    }

    #[test]
    fn label_safe_sanitizes_and_falls_back() {
        assert_eq!(Spawner::label_safe("Abc_123.Def"), "abc_123.def");
        assert_eq!(Spawner::label_safe("a b/c"), "a-b-c");
        assert_eq!(Spawner::label_safe("---"), "session");
        assert_eq!(Spawner::label_safe(""), "session");
        assert!(Spawner::label_safe(&"x".repeat(100)).len() <= 63);
    }
}
