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
use std::sync::Arc;

use bollard::Docker;
use bollard::container::{
    Config as ContainerConfig, CreateContainerOptions, RemoveContainerOptions,
    StartContainerOptions,
};
use bollard::errors::Error as DockerError;
use bollard::models::{ContainerStateStatusEnum, HostConfig};
use cctui_dispatcher_core::{
    Dispatcher, HandleState, SpawnOutcome, build_env, dedup_source, label_safe, worker_name,
};
use cctui_proto::worker_env::check_payload_env;
use cctui_proto::ws::WireDispatchSpec;

use crate::config::{Confinement, Egress};
use crate::exits::{self, Exit, Exits};

const LABEL_ORIGIN: &str = "cctui.dev/origin";
const ORIGIN: &str = "cctui-docker-dispatcher";
const LABEL_SESSION_ID: &str = "cctui.dev/session-id";

pub struct Spawner {
    image: String,
    network: Option<String>,
    cctui_url: String,
    mounts: Vec<String>,
    confinement: Confinement,
    docker: Docker,
    exits: Arc<Exits>,
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
        confinement: Confinement,
    ) -> anyhow::Result<Self> {
        let docker = match docker_host {
            Some(host) => Docker::connect_with_http(host, 60, bollard::API_DEFAULT_VERSION)?,
            None => Docker::connect_with_local_defaults()?,
        };
        docker.ping().await?;
        let exits = Arc::new(Exits::default());
        exits::watch(docker.clone(), Arc::clone(&exits), format!("{LABEL_ORIGIN}={ORIGIN}"));
        if confinement.egress == Egress::Forward {
            tracing::warn!(
                "egress=forward: cells get no NET_ADMIN and agent egress is only a proxy env the \
                 agent can ignore"
            );
        }
        if !confinement.landlock_strict {
            tracing::warn!(
                "landlock_strict=false: cells start without Landlock where the host lacks it"
            );
        }
        Ok(Self { image, network, cctui_url, mounts, confinement, docker, exits })
    }

    fn worker_env(&self, spec: &WireDispatchSpec) -> anyhow::Result<Vec<String>> {
        Self::build_worker_env(spec, &self.cctui_url, self.confinement)
    }

    /// The container name a dispatch maps to and its env.
    ///
    /// Errors wherever `dispatch` errors before it tries to create anything.
    fn admit(&self, spec: &WireDispatchSpec) -> anyhow::Result<(String, Vec<String>)> {
        if spec.session_id.is_empty() {
            anyhow::bail!("session_id is required");
        }
        Ok((worker_name(dedup_source(spec)), self.worker_env(spec)?))
    }

    /// The cell env: the contract vars, then the dispatcher's own confinement
    /// settings, which a payload cannot carry (`check_payload_env`).
    fn build_worker_env(
        spec: &WireDispatchSpec,
        cctui_url: &str,
        confinement: Confinement,
    ) -> anyhow::Result<Vec<String>> {
        check_payload_env(&spec.payload).map_err(anyhow::Error::msg)?;
        let base = build_env(spec, cctui_url)?;
        let mut env = base.env;
        if let Some(k) = base.machine_key {
            env.push(format!("CCTUI_MACHINE_KEY={k}"));
        }
        env.push(format!("WORKER_NET_MODE={}", confinement.egress.net_mode()));
        if !confinement.landlock_strict {
            env.push("CCTUI_LANDLOCK_STRICT=0".to_owned());
        }
        Ok(env)
    }

    fn discovery_labels(session_id: &str) -> HashMap<String, String> {
        let mut labels = HashMap::new();
        labels.insert(LABEL_ORIGIN.to_owned(), ORIGIN.to_owned());
        labels.insert(LABEL_SESSION_ID.to_owned(), label_safe(session_id));
        labels
    }

    /// `NET_ADMIN` is the only capability ever added, and only for transparent
    /// egress: the root entrypoint needs it to install the redirect, and the
    /// supervisor clears every capability set before the agent runs.
    fn host_config(mounts: &[String], network: Option<&str>, egress: Egress) -> HostConfig {
        HostConfig {
            auto_remove: Some(true),
            binds: if mounts.is_empty() { None } else { Some(mounts.to_vec()) },
            network_mode: network.map(ToOwned::to_owned),
            cap_add: (egress == Egress::Transparent).then(|| vec!["NET_ADMIN".to_owned()]),
            ..Default::default()
        }
    }
}

impl Dispatcher for Spawner {
    fn kind(&self) -> &'static str {
        "docker"
    }

    /// Spawn a worker container for the session. Idempotent: a repeat dispatch
    /// of the same key reuses the deterministic name; a name held by a live
    /// worker is reported as `deduplicated` rather than clobbered, and one held
    /// by a container that never started or already ended is replaced.
    async fn dispatch(&self, spec: &WireDispatchSpec) -> anyhow::Result<SpawnOutcome> {
        let (name, env) = self.admit(spec)?;

        let labels = Self::discovery_labels(&spec.session_id);
        let host_config =
            Self::host_config(&self.mounts, self.network.as_deref(), self.confinement.egress);

        let config = ContainerConfig {
            image: Some(self.image.clone()),
            env: Some(env),
            labels: Some(labels),
            host_config: Some(host_config),
            ..Default::default()
        };

        match self.create(&name, config.clone()).await {
            Ok(()) => {}
            Err(DockerError::DockerResponseServerError { status_code: 409, .. }) => {
                if self.holder_is_live(&name).await? {
                    return Ok(deduplicated(&name));
                }
                self.remove(&name).await?;
                self.create(&name, config)
                    .await
                    .map_err(|e| anyhow::anyhow!("recreating stale container {name}: {e}"))?;
            }
            Err(e) => anyhow::bail!("creating container: {e}"),
        }

        if let Err(e) =
            self.docker.start_container(&name, None::<StartContainerOptions<String>>).await
        {
            if let Err(cleanup) = self.remove(&name).await {
                tracing::warn!(container = %name, error = %cleanup, "removing the unstarted container failed");
            }
            anyhow::bail!("starting container: {e}");
        }

        Ok(SpawnOutcome {
            handle: format!("container/{name}"),
            status: "dispatched".to_owned(),
            namespace: None,
            exposure: None,
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
                if state.and_then(|s| s.status) == Some(ContainerStateStatusEnum::CREATED) {
                    return Ok((HandleState::Failed, Some("container never started".to_owned())));
                }
                let oom = state.and_then(|s| s.oom_killed).unwrap_or(false);
                let exit = state.and_then(|s| s.exit_code).unwrap_or(0);
                let memory = c.host_config.as_ref().and_then(|h| h.memory);
                Ok(exits::verdict(oom, exit, memory))
            }
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
                Ok(self.exits.ended(name).map_or((HandleState::Gone, None), Exit::state))
            }
            Err(e) => anyhow::bail!("inspecting container {name}: {e}"),
        }
    }

    async fn cancel(&self, handle: &str) -> anyhow::Result<()> {
        self.remove(handle.strip_prefix("container/").unwrap_or(handle)).await
    }

    fn supports_dedup_check(&self) -> bool {
        true
    }

    async fn dedup_check(&self, spec: &WireDispatchSpec) -> anyhow::Result<Option<SpawnOutcome>> {
        let (name, _) = self.admit(spec)?;
        Ok(self.holder_is_live(&name).await?.then(|| deduplicated(&name)))
    }
}

impl Spawner {
    async fn create(&self, name: &str, config: ContainerConfig<String>) -> Result<(), DockerError> {
        let opts = CreateContainerOptions { name: name.to_owned(), platform: None };
        self.docker.create_container(Some(opts), config).await.map(drop)
    }

    /// Force-remove `name`; one already gone counts as removed.
    async fn remove(&self, name: &str) -> anyhow::Result<()> {
        let opts = RemoveContainerOptions { force: true, ..Default::default() };
        match self.docker.remove_container(name, Some(opts)).await {
            Ok(()) | Err(DockerError::DockerResponseServerError { status_code: 404, .. }) => Ok(()),
            Err(e) => anyhow::bail!("removing container {name}: {e}"),
        }
    }

    /// Whether a container holding `name` is a run a repeat dispatch must not
    /// replace.
    async fn holder_is_live(&self, name: &str) -> anyhow::Result<bool> {
        match self.docker.inspect_container(name, None).await {
            Ok(c) => Ok(holds_a_live_run(c.state.and_then(|s| s.status))),
            Err(DockerError::DockerResponseServerError { status_code: 404, .. }) => Ok(false),
            Err(e) => anyhow::bail!("inspecting container {name}: {e}"),
        }
    }
}

/// A container that never started, or whose process already ended, holds the
/// name without a run behind it.
const fn holds_a_live_run(status: Option<ContainerStateStatusEnum>) -> bool {
    !matches!(
        status,
        Some(
            ContainerStateStatusEnum::CREATED
                | ContainerStateStatusEnum::EXITED
                | ContainerStateStatusEnum::DEAD
        )
    )
}

fn deduplicated(name: &str) -> SpawnOutcome {
    SpawnOutcome {
        handle: format!("container/{name}"),
        status: "deduplicated".to_owned(),
        namespace: None,
        exposure: None,
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
            traceparent: None,
        }
    }

    fn default_env(s: &WireDispatchSpec) -> anyhow::Result<Vec<String>> {
        Spawner::build_worker_env(s, "https://cctui.example.test", Confinement::default())
    }

    #[test]
    fn host_config_is_unprivileged_and_auto_removes() {
        for egress in [Egress::Transparent, Egress::Forward] {
            let hc = Spawner::host_config(&[], None, egress);
            assert_eq!(hc.privileged, None, "must never request a privileged container");
            assert_eq!(hc.auto_remove, Some(true));
            assert_eq!(hc.binds, None, "no mounts => no binds");
            assert_eq!(hc.network_mode, None, "no host network / no explicit network");
            assert_eq!(hc.pid_mode, None, "no host PID namespace");
            assert_eq!(hc.ipc_mode, None, "no host IPC namespace");
            assert_eq!(hc.userns_mode, None, "no host user namespace");
            assert_eq!(hc.security_opt, None, "default seccomp/apparmor profiles kept");
            assert_eq!(hc.devices, None, "no host devices");
        }
    }

    #[test]
    fn transparent_egress_adds_exactly_net_admin() {
        let hc = Spawner::host_config(&[], None, Egress::default());
        assert_eq!(hc.cap_add, Some(vec!["NET_ADMIN".to_owned()]), "default cell gets NET_ADMIN");
        assert_eq!(hc.cap_drop, None);
    }

    #[test]
    fn forward_egress_adds_no_capability() {
        let hc = Spawner::host_config(&[], None, Egress::Forward);
        assert_eq!(hc.cap_add, None);
    }

    #[test]
    fn the_cell_env_pins_the_egress_mode() {
        let s = spec("sess-net", json!({ "flow": "review" }));
        let env = default_env(&s).unwrap();
        assert!(env.contains(&"WORKER_NET_MODE=transparent".to_owned()), "{env:?}");
        assert!(env.iter().all(|e| !e.starts_with("CCTUI_LANDLOCK_STRICT=")), "strict by default");

        let forward = Confinement { egress: Egress::Forward, ..Confinement::default() };
        let env = Spawner::build_worker_env(&s, "https://c.example.test", forward).unwrap();
        assert!(env.contains(&"WORKER_NET_MODE=forward".to_owned()), "{env:?}");
    }

    #[test]
    fn best_effort_landlock_is_the_dispatchers_choice_only() {
        let lax = Confinement { landlock_strict: false, ..Confinement::default() };
        let s = spec("sess-ll", json!({ "flow": "review" }));
        let env = Spawner::build_worker_env(&s, "https://c.example.test", lax).unwrap();
        assert!(env.contains(&"CCTUI_LANDLOCK_STRICT=0".to_owned()), "{env:?}");

        for key in ["CCTUI_LANDLOCK_STRICT", "WORKER_NET_MODE", "CCTUI_GUARD_EXEC"] {
            let s = spec("sess-ll", json!({ "env": { key: "0" } }));
            let err = default_env(&s).unwrap_err();
            assert!(err.to_string().contains(key), "a payload must not set {key}: {err}");
        }
    }

    #[test]
    fn host_config_preserves_read_only_binds() {
        let mounts =
            vec!["/host/cache:/cache:ro".to_owned(), "/host/certs:/etc/ssl/certs:ro".to_owned()];
        let hc = Spawner::host_config(&mounts, Some("cctui-net"), Egress::Transparent);
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
        let env = default_env(&s).unwrap();
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
        let env = default_env(&s).unwrap();
        assert!(env.iter().all(|e| !e.starts_with("CCTUI_MACHINE_KEY=")));
    }

    #[test]
    fn worker_env_rejects_reserved_payload_env() {
        for key in ["CCTUI_URL", "CCTUI_MACHINE_KEY", "LD_PRELOAD", "HTTPS_PROXY"] {
            let s = spec("sess-r", json!({ "env": { key: "https://attacker.example" } }));
            let err = default_env(&s).unwrap_err();
            assert!(err.to_string().contains(key), "{err}");
        }
    }

    /// The request line and headers, with any body read off the socket.
    async fn read_request(tcp: &mut tokio::net::TcpStream) -> Option<String> {
        use tokio::io::AsyncReadExt;
        let mut buf = Vec::new();
        let mut chunk = [0_u8; 4096];
        let end = loop {
            if let Some(at) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break at + 4;
            }
            match tcp.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        };
        let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
        let length = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok());
        let chunked = head.contains("transfer-encoding: chunked");
        loop {
            let body = &buf[end..];
            let done = length
                .map_or_else(|| !chunked || body.ends_with(b"0\r\n\r\n"), |len| body.len() >= len);
            if done {
                return Some(String::from_utf8_lossy(&buf[..end]).into_owned());
            }
            match tcp.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
    }

    /// A stand-in docker API where a container named `held` exists or not.
    async fn fake_docker(held: Option<String>) -> Spawner {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                let held = held.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut tcp).await else { return };
                    let mut words = request.split_whitespace();
                    let method = words.next().unwrap_or_default();
                    let path = words.next().unwrap_or_default();
                    let exists = held.as_deref().is_some_and(|n| path.contains(n));
                    let (status, body) = match method {
                        "GET" if exists => ("200 OK", r#"{"Id":"c0","State":{"Running":true}}"#),
                        "POST" if path.contains("/containers/create") && held.is_some() => {
                            ("409 Conflict", r#"{"message":"name in use"}"#)
                        }
                        "POST" if path.contains("/containers/create") => {
                            ("201 Created", r#"{"Id":"c1","Warnings":[]}"#)
                        }
                        "POST" if path.ends_with("/start") => ("204 No Content", ""),
                        _ => ("404 Not Found", r#"{"message":"No such container"}"#),
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = tcp.write_all(response.as_bytes()).await;
                    let _ = tcp.shutdown().await;
                });
            }
        });

        Spawner {
            image: "example.com/worker:latest".into(),
            network: None,
            cctui_url: "https://cctui.example.test".into(),
            mounts: Vec::new(),
            confinement: Confinement::default(),
            docker: Docker::connect_with_http(&url, 5, bollard::API_DEFAULT_VERSION).unwrap(),
            exits: Arc::new(Exits::default()),
        }
    }

    #[derive(Default)]
    struct Daemon {
        status: Option<&'static str>,
        start_fails: bool,
        removals: usize,
    }

    /// A stand-in docker API holding one container through its lifecycle.
    async fn stateful_docker(daemon: Daemon) -> (Spawner, Arc<std::sync::Mutex<Daemon>>) {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let shared = Arc::new(std::sync::Mutex::new(daemon));
        let state = Arc::clone(&shared);
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                let Some(request) = read_request(&mut tcp).await else { continue };
                let mut words = request.split_whitespace();
                let method = words.next().unwrap_or_default().to_owned();
                let path = words.next().unwrap_or_default().to_owned();
                let (status, body) = {
                    let mut d = state.lock().unwrap();
                    let reply = match (method.as_str(), d.status) {
                        ("GET", Some(st)) => (
                            "200 OK",
                            format!(
                                r#"{{"Id":"c0","State":{{"Status":"{st}","Running":{}}}}}"#,
                                st == "running"
                            ),
                        ),
                        ("POST", Some(_)) if path.contains("/containers/create") => {
                            ("409 Conflict", r#"{"message":"name in use"}"#.to_owned())
                        }
                        ("POST", None) if path.contains("/containers/create") => {
                            d.status = Some("created");
                            ("201 Created", r#"{"Id":"c1","Warnings":[]}"#.to_owned())
                        }
                        ("POST", Some(_)) if path.ends_with("/start") && d.start_fails => (
                            "500 Internal Server Error",
                            r#"{"message":"bind source path does not exist"}"#.to_owned(),
                        ),
                        ("POST", Some(_)) if path.ends_with("/start") => {
                            d.status = Some("running");
                            ("204 No Content", String::new())
                        }
                        ("DELETE", Some(_)) => {
                            d.status = None;
                            d.removals += 1;
                            ("204 No Content", String::new())
                        }
                        _ => ("404 Not Found", r#"{"message":"No such container"}"#.to_owned()),
                    };
                    drop(d);
                    reply
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tcp.write_all(response.as_bytes()).await;
                let _ = tcp.shutdown().await;
            }
        });
        let spawner = Spawner {
            image: "example.com/worker:latest".into(),
            network: None,
            cctui_url: "https://cctui.example.test".into(),
            mounts: Vec::new(),
            confinement: Confinement::default(),
            docker: Docker::connect_with_http(&url, 5, bollard::API_DEFAULT_VERSION).unwrap(),
            exits: Arc::new(Exits::default()),
        };
        (spawner, shared)
    }

    fn keyed(key: &str) -> WireDispatchSpec {
        let mut s = spec("fresh-session", json!({}));
        s.dedup_key = Some(key.into());
        s
    }

    #[tokio::test]
    async fn a_failed_start_removes_the_container_so_a_retry_runs() {
        let s = keyed("triage-PROJ-2");
        let (spawner, daemon) =
            stateful_docker(Daemon { start_fails: true, ..Daemon::default() }).await;

        let err = spawner.dispatch(&s).await.unwrap_err();
        assert!(err.to_string().contains("starting container"), "{err}");
        assert_eq!(daemon.lock().unwrap().status, None, "the created container was left behind");
        assert!(spawner.dedup_check(&s).await.unwrap().is_none());

        daemon.lock().unwrap().start_fails = false;
        assert_eq!(spawner.dispatch(&s).await.unwrap().status, "dispatched");
        assert_eq!(daemon.lock().unwrap().status, Some("running"));
    }

    #[tokio::test]
    async fn a_name_held_by_a_container_with_no_run_is_replaced_not_deduplicated() {
        for stale in ["created", "exited", "dead"] {
            let s = keyed("triage-PROJ-3");
            let (spawner, daemon) =
                stateful_docker(Daemon { status: Some(stale), ..Daemon::default() }).await;
            assert!(spawner.dedup_check(&s).await.unwrap().is_none(), "{stale}");
            assert_eq!(spawner.dispatch(&s).await.unwrap().status, "dispatched", "{stale}");
            let held = {
                let d = daemon.lock().unwrap();
                (d.status, d.removals)
            };
            assert_eq!(held, (Some("running"), 1), "{stale}");
        }
    }

    #[tokio::test]
    async fn a_name_held_by_a_live_run_still_deduplicates() {
        for live in ["running", "restarting", "paused"] {
            let s = keyed("triage-PROJ-4");
            let (spawner, daemon) =
                stateful_docker(Daemon { status: Some(live), ..Daemon::default() }).await;
            assert!(spawner.dedup_check(&s).await.unwrap().is_some(), "{live}");
            assert_eq!(spawner.dispatch(&s).await.unwrap().status, "deduplicated", "{live}");
            assert_eq!(daemon.lock().unwrap().removals, 0, "{live}");
        }
    }

    #[tokio::test]
    async fn a_container_that_never_started_reads_failed_not_complete() {
        let (spawner, _daemon) =
            stateful_docker(Daemon { status: Some("created"), ..Daemon::default() }).await;
        let (state, reason) = spawner.status("container/x").await.unwrap();
        assert_eq!(state, HandleState::Failed);
        assert_eq!(reason.as_deref(), Some("container never started"));
    }

    /// The check and the create path share one admission step and the same
    /// name rule: a held name is `deduplicated` onto the same handle by both,
    /// a free one lets the create go through while the check answers none, and
    /// a dispatch the create path refuses errors in the check too.
    #[tokio::test]
    async fn the_dedup_check_agrees_with_the_create_path() {
        let mut s = spec("fresh-session", json!({}));
        s.dedup_key = Some("triage-PROJ-1".into());
        let name = worker_name("triage-PROJ-1");

        let held = fake_docker(Some(name.clone())).await;
        let checked = held.dedup_check(&s).await.unwrap().expect("a held name deduplicates");
        let created = held.dispatch(&s).await.unwrap();
        assert_eq!(created.status, "deduplicated");
        assert_eq!(checked.handle, format!("container/{name}"));
        assert_eq!((&checked.handle, &checked.status), (&created.handle, &created.status));

        let free = fake_docker(None).await;
        assert!(free.dedup_check(&s).await.unwrap().is_none());
        assert_eq!(free.dispatch(&s).await.unwrap().status, "dispatched");

        let mut refused = s;
        refused.session_id = String::new();
        assert!(held.dedup_check(&refused).await.is_err());
        assert!(held.dispatch(&refused).await.is_err());
        assert!(held.supports_dedup_check());
    }
}
