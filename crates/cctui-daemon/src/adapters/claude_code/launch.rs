use serde_json::{Value, json};

use super::control::FLEET_SOURCE;

/// Excludes transport flags (`--print`, `--output-format`, …) and the prompt tail.
#[derive(Debug, Default, Clone)]
pub(super) struct LaunchArgs {
    pub session_id: Option<String>,
    pub resume_from: Option<String>,
    /// Adds `--fork-session` after `--resume`. Ignored when `resume_from` is `None`.
    pub fork: bool,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Already mapped to the claude flag value.
    pub permission_flag: Option<String>,
    pub mcp_config: Option<String>,
    pub settings_path: Option<String>,
    pub name: Option<String>,
    /// One `--plugin-dir` per entry; survives respawns like `--settings`.
    pub plugin_dirs: Vec<String>,
}

impl LaunchArgs {
    pub fn from_spec(
        spec: &cctui_proto::adapter::SessionSpec,
        settings_path: Option<String>,
    ) -> Self {
        let clean = |o: &Option<String>| {
            o.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
        };
        Self {
            model: clean(&spec.model),
            effort: clean(&spec.effort),
            permission_flag: spec.permission_mode.map(|m| m.claude_flag().to_owned()),
            settings_path,
            name: clean(&spec.name),
            ..Self::default()
        }
    }

    /// Fleet dispatch passes `--name` verbatim, as the claude daemon does.
    pub fn for_dispatch(spec: &cctui_proto::adapter::SessionSpec) -> Self {
        Self { name: spec.name.clone(), ..Self::from_spec(spec, None) }
    }

    pub fn to_argv(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(resume) = &self.resume_from {
            args.push("--resume".to_owned());
            args.push(resume.clone());
            if self.fork {
                args.push("--fork-session".to_owned());
            }
        }
        if let Some(id) = &self.session_id {
            args.push("--session-id".to_owned());
            args.push(id.clone());
        }
        args.push("--agent".to_owned());
        args.push("claude".to_owned());
        push_flag(&mut args, "--name", self.name.as_ref());
        push_flag(&mut args, "--permission-mode", self.permission_flag.as_ref());
        self.push_respawned(&mut args);
        args
    }

    /// The flags the claude daemon re-applies when it relaunches the worker
    /// (`/clear`, `/compact`, CLI upgrade).
    pub fn respawn_argv(&self) -> Vec<String> {
        let mut args = vec!["--agent".to_owned(), "claude".to_owned()];
        self.push_respawned(&mut args);
        args
    }

    fn push_respawned(&self, args: &mut Vec<String>) {
        push_flag(args, "--effort", self.effort.as_ref());
        push_flag(args, "--model", self.model.as_ref());
        push_flag(args, "--mcp-config", self.mcp_config.as_ref());
        push_flag(args, "--settings", self.settings_path.as_ref());
        for dir in &self.plugin_dirs {
            args.push("--plugin-dir".to_owned());
            args.push(dir.clone());
        }
    }
}

fn push_flag(args: &mut Vec<String>, flag: &str, value: Option<&String>) {
    if let Some(value) = value {
        args.push(flag.to_owned());
        args.push(value.clone());
    }
}

/// The identity of one dispatched worker. `short` and `nonce` both satisfy the
/// claude daemon's `/^[a-f0-9]{8}$/` validator.
#[derive(Debug, Clone)]
pub(super) struct JobIds {
    pub session_id: String,
    pub short: String,
    pub nonce: String,
    pub created_at_ms: u64,
}

impl JobIds {
    /// Re-dispatch an existing job under a fresh nonce.
    pub fn existing(short: &str, session_id: String) -> Self {
        Self { session_id, short: short.to_owned(), nonce: nonce(), created_at_ms: now_ms() }
    }
}

/// Mint ids for a new worker, keeping a server-pre-minted session id so the
/// id the server bound the gateway token to is the one the worker registers.
pub(super) fn job_ids(forced: Option<&str>) -> JobIds {
    let session_id = forced.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let short = session_id[..8].to_owned();
    JobIds { session_id, short, nonce: nonce(), created_at_ms: now_ms() }
}

pub(super) fn nonce() -> String {
    uuid::Uuid::new_v4().simple().to_string().chars().take(8).collect()
}

fn now_ms() -> u64 {
    u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis()),
    )
    .unwrap_or(0)
}

/// The claude daemon's `dispatch` op. `env` is mirrored into `reattachEnv` so
/// the daemon reapplies it on any internal respawn; it never reaches `seed`,
/// `launch.args` or `state.json`.
pub(super) fn dispatch_request(
    ids: &JobIds,
    cwd: &str,
    launch: &LaunchArgs,
    prompt: Option<&str>,
    env: &std::collections::BTreeMap<String, String>,
    seed: &Value,
) -> Value {
    let mut args = launch.to_argv();
    if let Some(prompt) = prompt {
        args.push("--".to_owned());
        args.push(prompt.to_owned());
    }
    json!({
        "proto": 1,
        "op": "dispatch",
        "timeoutMs": 15000,
        "d": {
            "proto": 1,
            "short": ids.short,
            "nonce": ids.nonce,
            "sessionId": ids.session_id,
            "createdAt": ids.created_at_ms,
            "source": FLEET_SOURCE,
            "cwd": cwd,
            "launch": { "mode": "prompt", "args": args },
            "env": env,
            "reattachEnv": env,
            "isolation": "none",
            "respawnFlags": launch.respawn_argv(),
            "agent": "claude",
            "seed": seed,
            "cols": 120,
            "rows": 40,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_proto::adapter::{AdapterId, PermissionMode, SessionSpec};

    fn base_spec() -> SessionSpec {
        SessionSpec {
            service_tier: None,
            adapter_id: AdapterId::new("claude-code"),
            working_dir: None,
            prompt: None,
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            env: std::collections::BTreeMap::new(),
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
        }
    }

    #[test]
    fn plugin_dirs_ride_both_argv_and_respawn_flags() {
        let launch = LaunchArgs {
            settings_path: Some("/cfg/s.json".to_owned()),
            plugin_dirs: vec!["/cache/a/h1".to_owned(), "/cache/b/h2".to_owned()],
            ..LaunchArgs::default()
        };
        let tail = [
            "--settings",
            "/cfg/s.json",
            "--plugin-dir",
            "/cache/a/h1",
            "--plugin-dir",
            "/cache/b/h2",
        ];
        assert!(launch.to_argv().ends_with(&tail.map(str::to_owned)));
        assert!(launch.respawn_argv().ends_with(&tail.map(str::to_owned)));
        assert!(!LaunchArgs::default().to_argv().iter().any(|a| a == "--plugin-dir"));
    }

    #[test]
    fn launch_args_argv_order() {
        let spec = SessionSpec {
            working_dir: Some("/tmp/x".into()),
            prompt: Some("do it".into()),
            name: Some("task".into()),
            permission_mode: Some(PermissionMode::Auto),
            effort: Some("high".into()),
            model: Some("opus".into()),
            ..base_spec()
        };
        let mut la = LaunchArgs::from_spec(&spec, Some("/run/hook.json".into()));
        la.session_id = Some("sid-1".into());
        assert_eq!(
            la.to_argv(),
            vec![
                "--session-id",
                "sid-1",
                "--agent",
                "claude",
                "--name",
                "task",
                "--permission-mode",
                "acceptEdits",
                "--effort",
                "high",
                "--model",
                "opus",
                "--settings",
                "/run/hook.json",
            ]
        );
    }

    #[test]
    fn launch_args_fork_emits_resume_and_fork_session() {
        let mut la = LaunchArgs::from_spec(&base_spec(), None);
        la.resume_from = Some("parent-sid".into());
        la.fork = true;
        la.session_id = Some("child-sid".into());
        assert_eq!(
            la.to_argv(),
            vec![
                "--resume",
                "parent-sid",
                "--fork-session",
                "--session-id",
                "child-sid",
                "--agent",
                "claude",
            ]
        );
    }

    #[test]
    fn launch_args_blank_fields_omitted() {
        let spec =
            SessionSpec { model: Some("   ".into()), effort: Some(String::new()), ..base_spec() };
        let la = LaunchArgs::from_spec(&spec, None);
        assert!(la.model.is_none());
        assert!(la.effort.is_none());
        assert_eq!(la.to_argv(), vec!["--agent", "claude"]);
    }

    #[test]
    fn for_dispatch_keeps_name_verbatim() {
        let spec = SessionSpec { name: Some(" n ".into()), ..base_spec() };
        assert_eq!(LaunchArgs::for_dispatch(&spec).name.as_deref(), Some(" n "));
    }

    #[test]
    fn job_ids_honor_forced_session_id() {
        let ids = job_ids(Some("6e189420-f9a4-493f-b3d9-e0a80ac254c1"));
        assert_eq!(ids.session_id, "6e189420-f9a4-493f-b3d9-e0a80ac254c1");
        assert_eq!(ids.short, "6e189420");
        assert_eq!(ids.nonce.len(), 8);
        assert!(ids.nonce.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(ids.created_at_ms > 0);
        let fresh = job_ids(None);
        assert_eq!(&fresh.session_id[..8], fresh.short);
    }
}
