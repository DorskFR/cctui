//! Apple `container` spawn mechanics for the standalone apple dispatcher.
//!
//! Image-based boot from an OCI image via `container run` (no clone/snapshot —
//! Apple `container` has none, CCT-280). Deterministic
//! `cctui-worker-<sha1(dedup)[:12]>` naming for idempotency, env injection with
//! `cctui_machine_key` lifted out of the payload (CCT-191) and delivered as a
//! **mounted file** by default (CCT-245), optional repo mount + shallow-pull
//! signal, and lifecycle via `inspect`/`stop`/`delete`.
//!
//! All runtime calls go through [`ContainerCli`] so the mechanics are unit
//! tested without the macOS-only binary.
//!
//! ⚠️ Repo is PUBLIC — no homelab-specific images/hosts/networks here; the image
//! + host come from the dispatcher's own config.
#![allow(clippy::doc_markdown)]

use std::path::PathBuf;

use cctui_proto::ws::WireDispatchSpec;
use sha1::{Digest, Sha1};

use crate::cli::ContainerCli;

/// Lifecycle state of a spawned container handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A machine-key secret staged on the host, ready to mount into the guest.
#[derive(Debug, Clone)]
struct SecretMount {
    host_file: PathBuf,
    guest_path: String,
}

pub struct Spawner<C: ContainerCli> {
    cli: C,
    image: String,
    network: Option<String>,
    cctui_url: String,
    mounts: Vec<String>,
    repo_mount: Option<String>,
    secret_mount_path: String,
    secret_dir: PathBuf,
    secret_via_env: bool,
}

impl<C: ContainerCli> Spawner<C> {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        cli: C,
        image: String,
        network: Option<String>,
        cctui_url: String,
        mounts: Vec<String>,
        repo_mount: Option<String>,
        secret_mount_path: String,
        secret_dir: PathBuf,
        secret_via_env: bool,
    ) -> Self {
        Self {
            cli,
            image,
            network,
            cctui_url,
            mounts,
            repo_mount,
            secret_mount_path,
            secret_dir,
            secret_via_env,
        }
    }

    /// `cctui-worker-<sha1(dedup)[:12]>` — deterministic so a repeat dispatch of
    /// the same key maps to the same container (idempotency key).
    fn container_name(dedup_source: &str) -> String {
        let digest = Sha1::digest(dedup_source.as_bytes());
        format!("cctui-worker-{}", &hex::encode(digest)[..12])
    }

    /// The string the container name derives from: the caller's `dedup_key` when
    /// present, else the `session_id` (CCT-522). Mirrors the docker/kube
    /// dispatchers so `session_id` can be fresh per dispatch while a repeat of
    /// the same logical key still coalesces onto one container.
    fn dedup_source(spec: &WireDispatchSpec) -> &str {
        spec.dedup_key.as_deref().filter(|k| !k.is_empty()).unwrap_or(&spec.session_id)
    }

    /// The guest path a `host:guest[:ro]` mount exposes.
    fn mount_guest_path(mount: &str) -> Option<&str> {
        mount.split(':').nth(1).filter(|s| !s.is_empty())
    }

    /// Build the env passed to the worker, minus the machine key (which becomes
    /// a mounted file unless `secret_via_env`). Returns `(env, machine_key)`; the
    /// key is `None` when the payload carried none.
    fn build_env(&self, spec: &WireDispatchSpec) -> anyhow::Result<(Vec<String>, Option<String>)> {
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
        if let Some(u) = &spec.reply_url {
            env.push(format!("REPLY_URL={u}"));
        }
        if let Some(guest) = self.repo_mount.as_deref().and_then(Self::mount_guest_path) {
            env.push(format!("CCTUI_REPO_PATH={guest}"));
            env.push("CCTUI_GIT_SHALLOW=1".to_owned());
        }
        Ok((env, machine_key))
    }

    /// Assemble the full `container run` argv. Pure — no host/runtime side
    /// effects — so command construction is unit tested directly.
    fn build_run_args(
        &self,
        spec: &WireDispatchSpec,
        name: &str,
        secret: Option<&SecretMount>,
    ) -> anyhow::Result<Vec<String>> {
        let (mut env, machine_key) = self.build_env(spec)?;

        let mut args =
            vec!["run".to_owned(), "-d".to_owned(), "--name".to_owned(), name.to_owned()];
        if let Some(net) = &self.network {
            args.push("--network".to_owned());
            args.push(net.clone());
        }

        match (self.secret_via_env, machine_key) {
            (true, Some(k)) => env.push(format!("CCTUI_MACHINE_KEY={k}")),
            (false, Some(_)) => {
                let secret = secret.ok_or_else(|| {
                    anyhow::anyhow!("machine key present but no secret mount staged")
                })?;
                env.push(format!("CCTUI_MACHINE_KEY_FILE={}", secret.guest_path));
            }
            (_, None) => {}
        }

        for e in env {
            args.push("-e".to_owned());
            args.push(e);
        }
        if let Some(secret) = secret.filter(|_| !self.secret_via_env) {
            args.push("-v".to_owned());
            args.push(format!("{}:{}:ro", secret.host_file.display(), secret.guest_path));
        }
        if let Some(repo) = &self.repo_mount {
            args.push("-v".to_owned());
            args.push(repo.clone());
        }
        for m in &self.mounts {
            args.push("-v".to_owned());
            args.push(m.clone());
        }
        args.push(self.image.clone());
        Ok(args)
    }

    /// Stage the machine key as a 0600 host file to be mounted read-only into the
    /// guest. Preferred over an env var (a token in `container inspect` / the
    /// guest process list is visible, CCT-245).
    fn stage_secret(&self, name: &str, key: &str) -> anyhow::Result<SecretMount> {
        std::fs::create_dir_all(&self.secret_dir)?;
        let host_file = self.secret_dir.join(format!("{name}.key"));
        std::fs::write(&host_file, key)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&host_file, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(SecretMount { host_file, guest_path: self.secret_mount_path.clone() })
    }

    fn payload_machine_key(spec: &WireDispatchSpec) -> Option<String> {
        spec.payload.get("cctui_machine_key").and_then(|v| v.as_str()).map(ToOwned::to_owned)
    }

    /// Spawn a worker container for the session. Idempotent: a repeat dispatch of
    /// the same key reuses the deterministic name; `container run` failing
    /// because that name is already in use is reported as `deduplicated` rather
    /// than clobbering the running worker.
    pub async fn dispatch(&self, spec: &WireDispatchSpec) -> anyhow::Result<SpawnOutcome> {
        if spec.session_id.is_empty() {
            anyhow::bail!("session_id is required");
        }
        let name = Self::container_name(Self::dedup_source(spec));

        let secret = match (self.secret_via_env, Self::payload_machine_key(spec)) {
            (false, Some(k)) => Some(self.stage_secret(&name, &k)?),
            _ => None,
        };

        let args = self.build_run_args(spec, &name, secret.as_ref())?;
        let out = self.cli.exec(args).await?;
        if out.ok() {
            return Ok(SpawnOutcome {
                handle: format!("container/{name}"),
                status: "dispatched".to_owned(),
            });
        }
        if Self::is_name_in_use(&out.stderr) {
            return Ok(SpawnOutcome {
                handle: format!("container/{name}"),
                status: "deduplicated".to_owned(),
            });
        }
        anyhow::bail!("`container run` failed ({:?}): {}", out.code, out.stderr.trim());
    }

    /// Apple `container` reports a name collision on the stderr; match a couple
    /// of plausible phrasings so a repeat dispatch dedups instead of erroring.
    fn is_name_in_use(stderr: &str) -> bool {
        let s = stderr.to_ascii_lowercase();
        s.contains("already exists") || s.contains("already in use") || s.contains("name is in use")
    }

    fn name_of(handle: &str) -> &str {
        handle.strip_prefix("container/").unwrap_or(handle)
    }

    /// Lifecycle of a container handle, plus a human reason when it FAILED — a
    /// non-zero exit. The server lifts the reason into the completion webhook's
    /// `error` (CCT-429).
    pub async fn status(&self, handle: &str) -> anyhow::Result<(HandleState, Option<String>)> {
        let name = Self::name_of(handle);
        let out = self.cli.exec(vec!["inspect".to_owned(), name.to_owned()]).await?;
        if !out.ok() {
            // A missing container inspects with a non-zero exit; treat as gone.
            if Self::is_not_found(&out.stderr) {
                return Ok((HandleState::Gone, None));
            }
            anyhow::bail!("`container inspect` failed ({:?}): {}", out.code, out.stderr.trim());
        }
        Self::parse_inspect_state(&out.stdout)
    }

    fn is_not_found(stderr: &str) -> bool {
        let s = stderr.to_ascii_lowercase();
        s.contains("not found") || s.contains("no such") || s.contains("does not exist")
    }

    /// Map `container inspect` JSON to a lifecycle state. Apple `container`
    /// returns a JSON array of container records; each carries a `status`
    /// (`running`/`stopped`) and, when stopped, an `exitCode`.
    fn parse_inspect_state(stdout: &str) -> anyhow::Result<(HandleState, Option<String>)> {
        let v: serde_json::Value = serde_json::from_str(stdout.trim())
            .map_err(|e| anyhow::anyhow!("parsing `container inspect` json: {e}"))?;
        let record = match &v {
            serde_json::Value::Array(a) => {
                a.first().ok_or_else(|| anyhow::anyhow!("empty inspect array"))?
            }
            other => other,
        };
        let status = record
            .get("status")
            .and_then(|s| s.as_str())
            .or_else(|| record.pointer("/state/status").and_then(|s| s.as_str()))
            .unwrap_or("unknown")
            .to_ascii_lowercase();
        match status.as_str() {
            "running" => Ok((HandleState::Running, None)),
            "stopped" | "exited" => {
                let exit = record
                    .get("exitCode")
                    .or_else(|| record.pointer("/state/exitCode"))
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(0);
                Ok(if exit == 0 {
                    (HandleState::Complete, None)
                } else {
                    (HandleState::Failed, Some(format!("container exited with code {exit}")))
                })
            }
            other => Ok((HandleState::Running, Some(format!("unknown status: {other}")))),
        }
    }

    /// Stop then delete the container (Apple `container` has no auto-remove). A
    /// missing container at either step is a successful cancel.
    pub async fn cancel(&self, handle: &str) -> anyhow::Result<()> {
        let name = Self::name_of(handle);
        let stop = self.cli.exec(vec!["stop".to_owned(), name.to_owned()]).await?;
        if !stop.ok() && !Self::is_not_found(&stop.stderr) {
            anyhow::bail!("`container stop` failed ({:?}): {}", stop.code, stop.stderr.trim());
        }
        let del = self.cli.exec(vec!["delete".to_owned(), name.to_owned()]).await?;
        if !del.ok() && !Self::is_not_found(&del.stderr) {
            anyhow::bail!("`container delete` failed ({:?}): {}", del.code, del.stderr.trim());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use serde_json::json;

    use super::*;
    use crate::cli::CliOutput;

    #[derive(Default)]
    struct MockCli {
        calls: Mutex<Vec<Vec<String>>>,
        responses: Mutex<Vec<CliOutput>>,
    }

    impl MockCli {
        fn with_responses(responses: Vec<CliOutput>) -> Self {
            Self { calls: Mutex::new(vec![]), responses: Mutex::new(responses) }
        }

        fn calls(&self) -> Vec<Vec<String>> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl ContainerCli for MockCli {
        async fn exec(&self, args: Vec<String>) -> anyhow::Result<CliOutput> {
            self.calls.lock().unwrap().push(args);
            let mut r = self.responses.lock().unwrap();
            Ok(if r.is_empty() {
                CliOutput { code: Some(0), stdout: String::new(), stderr: String::new() }
            } else {
                r.remove(0)
            })
        }
    }

    fn ok(stdout: &str) -> CliOutput {
        CliOutput { code: Some(0), stdout: stdout.to_owned(), stderr: String::new() }
    }

    fn err(code: i32, stderr: &str) -> CliOutput {
        CliOutput { code: Some(code), stdout: String::new(), stderr: stderr.to_owned() }
    }

    fn spawner(cli: MockCli) -> Spawner<MockCli> {
        Spawner::new(
            cli,
            "registry.example.test/cctui-worker:latest".to_owned(),
            None,
            "https://cctui.example.test".to_owned(),
            vec![],
            None,
            "/run/cctui/machine_key".to_owned(),
            std::env::temp_dir().join(format!("cctui-apple-test-{}", uuid::Uuid::new_v4())),
            false,
        )
    }

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
    fn container_name_is_deterministic_and_prefixed() {
        let a = Spawner::<MockCli>::container_name("session-xyz");
        let b = Spawner::<MockCli>::container_name("session-xyz");
        assert_eq!(a, b);
        assert!(a.starts_with("cctui-worker-"));
        assert_eq!(a.len(), "cctui-worker-".len() + 12);
        assert_ne!(a, Spawner::<MockCli>::container_name("session-abc"));
    }

    #[test]
    fn container_name_derives_from_dedup_key_so_session_id_can_be_fresh() {
        let mut s1 = spec("11111111-1111-4111-8111-111111111111", json!({}));
        s1.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        let mut s2 = spec("22222222-2222-4222-8222-222222222222", json!({}));
        s2.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        assert_eq!(
            Spawner::<MockCli>::container_name(Spawner::<MockCli>::dedup_source(&s1)),
            Spawner::<MockCli>::container_name(Spawner::<MockCli>::dedup_source(&s2)),
        );
    }

    #[test]
    fn build_run_args_injects_env_and_mounts_secret_file() {
        let sp = spawner(MockCli::default());
        let s = spec("sess-123", json!({ "name": "Review #7", "cctui_machine_key": "SECRET" }));
        let name = "cctui-worker-abc";
        let secret = SecretMount {
            host_file: PathBuf::from("/tmp/x.key"),
            guest_path: "/run/cctui/machine_key".to_owned(),
        };
        let args = sp.build_run_args(&s, name, Some(&secret)).unwrap();

        assert_eq!(&args[0..4], &["run", "-d", "--name", name]);
        assert!(args.contains(&"SESSION_ID=sess-123".to_owned()));
        assert!(args.contains(&"TASK_ID=sess-123".to_owned()));
        assert!(args.contains(&"TASK_NAME=Review #7".to_owned()));
        assert!(args.contains(&"CCTUI_URL=https://cctui.example.test".to_owned()));
        assert!(args.contains(&"REPLY_URL=https://cb.example.test".to_owned()));
        // Machine key is a mounted file, NOT a plain env var, and NOT in payload.
        assert!(args.contains(&"CCTUI_MACHINE_KEY_FILE=/run/cctui/machine_key".to_owned()));
        assert!(args.iter().all(|a| !a.starts_with("CCTUI_MACHINE_KEY=")));
        assert!(args.contains(&"/tmp/x.key:/run/cctui/machine_key:ro".to_owned()));
        let tp = args.iter().find(|a| a.starts_with("TASK_PAYLOAD_JSON=")).unwrap();
        assert!(!tp.contains("SECRET"), "machine key leaked into payload: {tp}");
        // The OCI image is the trailing positional argument.
        assert_eq!(args.last().unwrap(), "registry.example.test/cctui-worker:latest");
    }

    #[test]
    fn build_run_args_env_secret_mode_uses_plain_var_no_mount() {
        let mut sp = spawner(MockCli::default());
        sp.secret_via_env = true;
        let s = spec("sess-1", json!({ "cctui_machine_key": "SECRET" }));
        let args = sp.build_run_args(&s, "n", None).unwrap();
        assert!(args.contains(&"CCTUI_MACHINE_KEY=SECRET".to_owned()));
        assert!(args.iter().all(|a| a != "-v"));
    }

    #[test]
    fn build_run_args_adds_network_repo_and_extra_mounts() {
        let mut sp = spawner(MockCli::default());
        sp.network = Some("cctui-net".to_owned());
        sp.repo_mount = Some("/host/repo:/workspace/repo".to_owned());
        sp.mounts = vec!["/host/cache:/cache:ro".to_owned()];
        let s = spec("sess-2", json!({}));
        let args = sp.build_run_args(&s, "n", None).unwrap();
        let pos = |x: &str| args.iter().position(|a| a == x);
        assert!(pos("--network").is_some());
        assert!(args.contains(&"cctui-net".to_owned()));
        // Repo mount surfaces both as a volume and a shallow-pull signal.
        assert!(args.contains(&"/host/repo:/workspace/repo".to_owned()));
        assert!(args.contains(&"CCTUI_REPO_PATH=/workspace/repo".to_owned()));
        assert!(args.contains(&"CCTUI_GIT_SHALLOW=1".to_owned()));
        assert!(args.contains(&"/host/cache:/cache:ro".to_owned()));
    }

    #[tokio::test]
    async fn dispatch_runs_container_and_stages_secret_file() {
        let cli = MockCli::with_responses(vec![ok("")]);
        let sp = spawner(cli);
        let s = spec("sess-abc", json!({ "cctui_machine_key": "TOPSECRET" }));
        let out = sp.dispatch(&s).await.unwrap();
        assert_eq!(out.status, "dispatched");
        assert!(out.handle.starts_with("container/cctui-worker-"));
        let calls = sp.cli.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0][0], "run");
        // The staged secret file exists on the host with the key contents.
        let name = out.handle.strip_prefix("container/").unwrap();
        let host_file = sp.secret_dir.join(format!("{name}.key"));
        assert_eq!(std::fs::read_to_string(&host_file).unwrap(), "TOPSECRET");
        std::fs::remove_dir_all(&sp.secret_dir).ok();
    }

    #[tokio::test]
    async fn dispatch_dedups_on_name_in_use() {
        let cli = MockCli::with_responses(vec![err(1, "Error: container already exists")]);
        let sp = spawner(cli);
        let out = sp.dispatch(&spec("sess-d", json!({}))).await.unwrap();
        assert_eq!(out.status, "deduplicated");
    }

    #[tokio::test]
    async fn dispatch_surfaces_real_run_failure() {
        let cli = MockCli::with_responses(vec![err(125, "Error: no such image")]);
        let sp = spawner(cli);
        let e = sp.dispatch(&spec("sess-e", json!({}))).await.unwrap_err();
        assert!(e.to_string().contains("no such image"), "{e}");
    }

    #[tokio::test]
    async fn dispatch_requires_session_id() {
        let sp = spawner(MockCli::default());
        assert!(sp.dispatch(&spec("", json!({}))).await.is_err());
    }

    #[tokio::test]
    async fn status_maps_running_stopped_and_gone() {
        let cli = MockCli::with_responses(vec![
            ok(r#"[{"status":"running"}]"#),
            ok(r#"[{"status":"stopped","exitCode":0}]"#),
            ok(r#"[{"status":"stopped","exitCode":137}]"#),
            err(1, "Error: container not found"),
        ]);
        let sp = spawner(cli);
        assert_eq!(sp.status("container/x").await.unwrap().0, HandleState::Running);
        assert_eq!(sp.status("container/x").await.unwrap().0, HandleState::Complete);
        let (state, reason) = sp.status("container/x").await.unwrap();
        assert_eq!(state, HandleState::Failed);
        assert!(reason.unwrap().contains("137"));
        assert_eq!(sp.status("container/x").await.unwrap().0, HandleState::Gone);
    }

    #[tokio::test]
    async fn cancel_stops_then_deletes_and_tolerates_missing() {
        let cli = MockCli::with_responses(vec![
            err(1, "Error: not found"),
            err(1, "Error: no such container"),
        ]);
        let sp = spawner(cli);
        sp.cancel("container/gone").await.unwrap();
        let calls = sp.cli.calls();
        assert_eq!(calls[0][0], "stop");
        assert_eq!(calls[1][0], "delete");
    }
}
