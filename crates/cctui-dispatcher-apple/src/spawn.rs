//! Apple `container` spawn mechanics for the standalone apple dispatcher.
//!
//! Image-based boot from an OCI image via `container run` (no clone/snapshot —
//! Apple `container` has none). Deterministic
//! `cctui-worker-<sha1(dedup)[:12]>` naming for idempotency, env injection with
//! `cctui_machine_key` lifted out of the payload and delivered as a
//! **mounted file** by default (or a 0600 `--env-file`, never argv), optional
//! repo mount + shallow-pull
//! signal, and lifecycle via `inspect`/`stop`/`delete`.
//!
//! All runtime calls go through [`ContainerCli`] so the mechanics are unit
//! tested without the macOS-only binary.
//!
//! ⚠️ Repo is PUBLIC — no homelab-specific images/hosts/networks here; the image
//! + host come from the dispatcher's own config.
#![allow(clippy::doc_markdown)]

use std::path::PathBuf;

use cctui_dispatcher_core::{
    Dispatcher, HandleState, SpawnOutcome, build_env, dedup_source, worker_name,
};
use cctui_proto::worker_env::check_payload_env;
use cctui_proto::ws::WireDispatchSpec;

use crate::cli::ContainerCli;

/// A machine-key secret staged on the host: a file mounted into the guest, or
/// the `--env-file` in `secret_via_env` mode.
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

    /// The guest path a `host:guest[:ro]` mount exposes.
    fn mount_guest_path(mount: &str) -> Option<&str> {
        mount.split(':').nth(1).filter(|s| !s.is_empty())
    }

    /// Build the env passed to the worker, minus the machine key (a mounted
    /// file, or an env file with `secret_via_env`). Returns `(env, machine_key)`; the
    /// key is `None` when the payload carried none.
    fn worker_env(&self, spec: &WireDispatchSpec) -> anyhow::Result<(Vec<String>, Option<String>)> {
        check_payload_env(&spec.payload).map_err(anyhow::Error::msg)?;
        let base = build_env(spec, &self.cctui_url)?;
        let mut env = base.env;
        if let Some(guest) = self.repo_mount.as_deref().and_then(Self::mount_guest_path) {
            env.push(format!("CCTUI_REPO_PATH={guest}"));
            env.push("CCTUI_GIT_SHALLOW=1".to_owned());
        }
        Ok((env, base.machine_key))
    }

    /// Assemble the full `container run` argv. Pure — no host/runtime side
    /// effects — so command construction is unit tested directly.
    fn build_run_args(
        &self,
        spec: &WireDispatchSpec,
        name: &str,
        secret: Option<&SecretMount>,
    ) -> anyhow::Result<Vec<String>> {
        let (mut env, machine_key) = self.worker_env(spec)?;

        let mut args =
            vec!["run".to_owned(), "-d".to_owned(), "--name".to_owned(), name.to_owned()];
        if let Some(net) = &self.network {
            args.push("--network".to_owned());
            args.push(net.clone());
        }

        let secret = if machine_key.is_some() {
            Some(secret.ok_or_else(|| anyhow::anyhow!("machine key present but no secret file staged"))?)
        } else {
            None
        };
        if let Some(secret) = secret.filter(|_| !self.secret_via_env) {
            env.push(format!("CCTUI_MACHINE_KEY_FILE={}", secret.guest_path));
        }

        for e in env {
            args.push("-e".to_owned());
            args.push(e);
        }
        if let Some(secret) = secret.filter(|_| self.secret_via_env) {
            args.push("--env-file".to_owned());
            args.push(secret.host_file.display().to_string());
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

    /// Stage the machine key as a 0600 host file: mounted read-only into the
    /// guest, or passed as `--env-file` with `secret_via_env` so the key never
    /// reaches the `container run` argv.
    fn stage_secret(&self, name: &str, key: &str) -> anyhow::Result<SecretMount> {
        let (host_file, contents) = if self.secret_via_env {
            (self.secret_dir.join(format!("{name}.env")), format!("CCTUI_MACHINE_KEY={key}\n"))
        } else {
            (self.secret_dir.join(format!("{name}.key")), key.to_owned())
        };
        write_private(&self.secret_dir, &host_file, contents.as_bytes())?;
        Ok(SecretMount { host_file, guest_path: self.secret_mount_path.clone() })
    }

    fn payload_machine_key(spec: &WireDispatchSpec) -> Option<String> {
        spec.payload.get("cctui_machine_key").and_then(|v| v.as_str()).map(ToOwned::to_owned)
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
}

/// Write `path` as a 0600 file, never readable by others even briefly: the
/// bytes go to a fresh 0600 temp file that is then renamed over `path`.
fn write_private(dir: &std::path::Path, path: &std::path::Path, contents: &[u8]) -> anyhow::Result<()> {
    use std::io::Write;

    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        builder.mode(0o700);
        opts.mode(0o600);
    }
    builder.create(dir)?;
    let tmp = dir.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let written = opts.open(&tmp).and_then(|mut f| f.write_all(contents)).and_then(|()| {
        std::fs::rename(&tmp, path)
    });
    if written.is_err() {
        std::fs::remove_file(&tmp).ok();
    }
    written?;
    Ok(())
}

impl<C: ContainerCli> Dispatcher for Spawner<C> {
    fn kind(&self) -> &'static str {
        "apple"
    }

    /// Spawn a worker container for the session. Idempotent: a repeat dispatch of
    /// the same key reuses the deterministic name; `container run` failing
    /// because that name is already in use is reported as `deduplicated` rather
    /// than clobbering the running worker.
    async fn dispatch(&self, spec: &WireDispatchSpec) -> anyhow::Result<SpawnOutcome> {
        if spec.session_id.is_empty() {
            anyhow::bail!("session_id is required");
        }
        let name = worker_name(dedup_source(spec));

        let secret = match Self::payload_machine_key(spec) {
            Some(k) => Some(self.stage_secret(&name, &k)?),
            None => None,
        };

        let args = self.build_run_args(spec, &name, secret.as_ref())?;
        let out = self.cli.exec(args).await?;
        if out.ok() {
            return Ok(SpawnOutcome {
                handle: format!("container/{name}"),
                status: "dispatched".to_owned(),
                namespace: None,
            });
        }
        if Self::is_name_in_use(&out.stderr) {
            return Ok(SpawnOutcome {
                handle: format!("container/{name}"),
                status: "deduplicated".to_owned(),
                namespace: None,
            });
        }
        anyhow::bail!("`container run` failed ({:?}): {}", out.code, out.stderr.trim());
    }

    /// Lifecycle of a container handle, plus a human reason when it FAILED — a
    /// non-zero exit. The server lifts the reason into the completion webhook's
    /// `error`.
    async fn status(&self, handle: &str) -> anyhow::Result<(HandleState, Option<String>)> {
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

    /// Stop then delete the container (Apple `container` has no auto-remove). A
    /// missing container at either step is a successful cancel.
    async fn cancel(&self, handle: &str) -> anyhow::Result<()> {
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
    fn container_name_derives_from_dedup_key_so_session_id_can_be_fresh() {
        let mut s1 = spec("11111111-1111-4111-8111-111111111111", json!({}));
        s1.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        let mut s2 = spec("22222222-2222-4222-8222-222222222222", json!({}));
        s2.dedup_key = Some("triage-PROJ-202606231511".to_owned());
        assert_eq!(worker_name(dedup_source(&s1)), worker_name(dedup_source(&s2)));
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
    fn build_run_args_env_secret_mode_uses_env_file_not_argv() {
        let mut sp = spawner(MockCli::default());
        sp.secret_via_env = true;
        let s = spec("sess-1", json!({ "cctui_machine_key": "SECRET" }));
        let secret = SecretMount {
            host_file: PathBuf::from("/tmp/n.env"),
            guest_path: "/run/cctui/machine_key".to_owned(),
        };
        let args = sp.build_run_args(&s, "n", Some(&secret)).unwrap();
        assert!(args.iter().all(|a| !a.contains("SECRET")), "machine key on argv: {args:?}");
        let at = args.iter().position(|a| a == "--env-file").expect("--env-file passed");
        assert_eq!(args[at + 1], "/tmp/n.env");
        assert!(args.iter().all(|a| !a.starts_with("CCTUI_MACHINE_KEY_FILE=")));
        assert!(args.iter().all(|a| a != "-v"));
    }

    #[test]
    fn build_run_args_env_secret_mode_requires_a_staged_env_file() {
        let mut sp = spawner(MockCli::default());
        sp.secret_via_env = true;
        let s = spec("sess-1", json!({ "cctui_machine_key": "SECRET" }));
        assert!(sp.build_run_args(&s, "n", None).is_err());
    }

    #[cfg(unix)]
    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[tokio::test]
    async fn dispatch_env_secret_mode_keeps_the_key_off_argv() {
        let mut sp = spawner(MockCli::with_responses(vec![ok("")]));
        sp.secret_via_env = true;
        let s = spec("sess-env", json!({ "cctui_machine_key": "TOPSECRET" }));
        let out = sp.dispatch(&s).await.unwrap();
        let calls = sp.cli.calls();
        assert!(calls[0].iter().all(|a| !a.contains("TOPSECRET")), "key on argv: {:?}", calls[0]);
        let name = out.handle.strip_prefix("container/").unwrap();
        let env_file = sp.secret_dir.join(format!("{name}.env"));
        assert!(calls[0].contains(&env_file.display().to_string()));
        assert_eq!(std::fs::read_to_string(&env_file).unwrap(), "CCTUI_MACHINE_KEY=TOPSECRET\n");
        #[cfg(unix)]
        assert_eq!(mode_of(&env_file), 0o600);
        std::fs::remove_dir_all(&sp.secret_dir).ok();
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
        #[cfg(unix)]
        assert_eq!(mode_of(&host_file), 0o600);
        std::fs::remove_dir_all(&sp.secret_dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn restaging_a_secret_replaces_a_lax_file_with_a_0600_one() {
        use std::os::unix::fs::PermissionsExt;
        let sp = spawner(MockCli::default());
        std::fs::create_dir_all(&sp.secret_dir).unwrap();
        let stale = sp.secret_dir.join("n.key");
        std::fs::write(&stale, "OLD").unwrap();
        std::fs::set_permissions(&stale, std::fs::Permissions::from_mode(0o644)).unwrap();
        let staged = sp.stage_secret("n", "NEW").unwrap();
        assert_eq!(staged.host_file, stale);
        assert_eq!(std::fs::read_to_string(&stale).unwrap(), "NEW");
        assert_eq!(mode_of(&stale), 0o600);
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

    #[test]
    fn build_run_args_rejects_reserved_payload_env() {
        let sp = spawner(MockCli::default());
        for key in ["CCTUI_URL", "CCTUI_MACHINE_KEY", "DYLD_INSERT_LIBRARIES", "PATH"] {
            let s = spec("sess-r", json!({ "env": { key: "x" } }));
            let err = sp.build_run_args(&s, "n", None).unwrap_err();
            assert!(err.to_string().contains(key), "{err}");
        }
    }
}
