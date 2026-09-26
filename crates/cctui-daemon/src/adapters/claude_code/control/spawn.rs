use super::{
    AdapterEvent, Context, DeferredDispatch, DispatchDoneTracker, Driver, Duration, EndReason,
    JobIds, LaunchArgs, LaunchEnv, LaunchGate, Path, PathBuf, StateJson, agent_relay_config,
    build_session_context, detect_whip_from_settings, dispatch_done, ensure_hook_settings, json,
    launch, mpsc, resolve_launch_env_for, socket, stage_uploads, transcript,
};

impl Driver {
    /// Resume-on-reply: if `short` has no live worker, revive it
    /// before a reply is delivered. The claude control socket cannot wake an
    /// exited job itself — `attach`/`reply` both return ENOJOB; the picker's
    /// "enter to resume" is client-side. What does work (probed against
    /// claude v2.1.162) is a `dispatch` that reuses the dead job's identity:
    /// same `short`, the `resumeSessionId` from its on-disk `state.json`, and
    /// `--resume <id>` as the launch argv — the daemon spawns a fresh worker
    /// bound to the saved conversation, original transcript re-pinned.
    ///
    /// No-op (one cheap `has` round-trip) when the worker is alive.
    pub(super) async fn resume_if_hibernated(
        &self,
        sock: &std::path::Path,
        short: &str,
        local_id: &str,
        env: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        self.resume_worker(sock, short, local_id, None, None, env).await
    }

    /// The single source of gateway-routing env for every worker (re)launch:
    /// pull it from the server's durable `sessions.account_id` binding
    /// so routing survives a daemon / claude-daemon restart and session-id
    /// rotation, instead of trusting whatever env the triggering command carried.
    ///
    /// `hint` is that carried env (spawn `spec.env`, reply/resume push) — used as
    /// a fallback only when the authoritative pull is unavailable (older server,
    /// transient network) so a rollout or blip degrades to the prior push
    /// behavior rather than failing.
    ///
    /// Fail-closed: when the server reports the session IS account-bound but the
    /// resolved env is empty (account gone / unmintable), refuse the launch — a
    /// worker started without the gateway credential would silently route to the
    /// default upstream and 401. Returning `Err` aborts the dispatch loudly
    /// instead of producing another silent auth drop.
    pub(super) async fn resolve_launch_env(
        &self,
        local_id: &str,
        hint: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<LaunchEnv> {
        match resolve_launch_env_for(
            self.server.as_ref(),
            self.machine_key.as_ref(),
            local_id,
            hint,
        )
        .await
        {
            Ok(launch) => Ok(launch),
            // Surface the fail-closed refusal as a visible failure state before
            // aborting, so the UI shows the account problem rather than the launch
            // silently dying.
            Err(e) => {
                self.emit(AdapterEvent::SessionEnded {
                    local_id: local_id.to_owned(),
                    reason: EndReason::Crashed {
                        detail: format!(
                            "account-bound session refused: the server returned no gateway \
                             credential (account missing/unmintable). The worker was NOT \
                             launched on ambient credentials — reconnect the account in cctui. \
                             ({e})"
                        ),
                    },
                })
                .await;
                Err(e)
            }
        }
    }

    /// Revive an exited worker bound to its saved conversation. Prefers the
    /// on-disk `state.json` (so `/clear`/`/compact`'s rotated `resumeSessionId`
    /// is honored); when it's gone — e.g. an archived session whose
    /// `claude rm` deleted the job metadata but left the transcript — falls back
    /// to the caller-supplied `(session_id, cwd)` from the server's DB row.
    /// No-op (one cheap `has` round-trip) when the worker is alive.
    pub(super) async fn resume_worker(
        &self,
        sock: &std::path::Path,
        short: &str,
        local_id: &str,
        fallback_session_id: Option<&str>,
        fallback_cwd: Option<&str>,
        env: &std::collections::BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        let alive = |resp: &serde_json::Value| {
            resp.get("alive").and_then(serde_json::Value::as_bool).unwrap_or(false)
        };
        let has = socket::one_shot(sock, &json!({"proto":1,"op":"has","short":short})).await?;
        if alive(&has) {
            return Ok(());
        }

        // Re-derive the gateway env + per-account settings from the server's
        // durable binding, falling back to the pushed `env` hint if the pull is
        // unavailable — a cold-resume must never relaunch the worker with empty
        // env (it would 401). Fail-closed inside `resolve_launch_env`.
        let launch = self.resolve_launch_env(local_id, env).await?;
        let env = launch.env;

        // Re-apply the managed hook settings on cold resume so the revived worker
        // keeps its ask/permission/Stop hooks AND picks up the (possibly
        // refreshed) per-account settings the env pull re-served.
        // `whip` is recovered from the settings file the original spawn wrote for
        // this `short` (its `hooks.Stop` block is whip-only) — cold resume has no
        // `spec` to read it from directly, and defaulting false would silently
        // downgrade a 🐎 session's enforcement profile.
        let whip = detect_whip_from_settings(short);
        let st = StateJson::read(&self.cfg.jobs_root, short);
        // Carry the gateway env + the session's model/effort (from `state.json`)
        // into the managed `--settings` file so a spare-claimed resume
        // keeps its routing env and model/effort — the settings file survives the
        // spare-claim, the dispatch `env` and a `--model` CLI arg do not.
        let settings_arg = ensure_hook_settings(
            &self.cfg.hook_socket_path,
            whip,
            short,
            launch.settings.as_ref(),
            &env,
            st.as_ref().and_then(|s| s.model.as_deref()),
            st.as_ref().and_then(|s| s.effort.as_deref()),
            launch.whip_phrases.as_ref(),
            None,
        )
        .map(|p| p.to_string_lossy().into_owned());

        // `/clear`/`/compact` rotate the live conversation into the id recorded
        // in `resumeSessionId`; resuming the stale spawn id would fork the
        // conversation back at the pre-reset state. When state.json is
        // gone, fall back to the id/cwd the server passed from its DB row.
        let session_id = st
            .as_ref()
            .and_then(|s| s.resume_session_id.clone().or_else(|| s.session_id.clone()))
            .or_else(|| fallback_session_id.map(str::to_owned))
            .ok_or_else(|| {
                anyhow::anyhow!("no session id on disk or from caller to resume {short}")
            })?;
        let cwd = st
            .as_ref()
            .and_then(|s| s.cwd.clone())
            .or_else(|| fallback_cwd.map(str::to_owned))
            .ok_or_else(|| anyhow::anyhow!("no cwd on disk or from caller to resume {short}"))?;

        // NB: resume deliberately does NOT pass `--model`/`--effort`.
        // Asserting `--model` on a `--resume` forces the claude
        // daemon down its spare-claim/cold relaunch, which does NOT reapply
        // cctui's dispatch gateway env (background workers don't inherit gateway
        // vars) — so the revived worker came up with no `ANTHROPIC_BASE_URL`/
        // token and 401ed/ConnectionRefused. The resumed session already carries
        // its model/effort in the transcript; only `spawn` seeds them as flags.
        let launch = LaunchArgs {
            settings_path: settings_arg,
            plugin_dirs: launch.plugin_dirs,
            ..resume_launch(&session_id)
        };
        let ids = JobIds::existing(short, session_id);
        // `state.json` already exists for this short; the daemon keeps its
        // identity fields, so the seed is just protocol filler.
        let seed =
            json!({ "intent": st.as_ref().and_then(|s| s.intent.clone()).unwrap_or_default() });
        let req = launch::dispatch_request(&ids, &cwd, &launch, None, &env, &seed);
        let session_id = ids.session_id;
        let resp: serde_json::Value = socket::call(sock, &req)
            .await
            .with_context(|| format!("resume dispatch for hibernated session {short}"))?;
        tracing::info!(?resp, %short, %session_id, "resumed hibernated session via dispatch");

        // Wait (bounded) for the revived worker to report alive, then give the
        // PTY a moment to finish booting so the reply isn't swallowed by a
        // half-started claude. The next poll tick re-adds the short to the
        // roster and the AttachManager's persistent attach keeps it awake.
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if let Ok(resp) =
                socket::one_shot(sock, &json!({"proto":1,"op":"has","short":short})).await
                && alive(&resp)
            {
                tokio::time::sleep(Duration::from_millis(1500)).await;
                return Ok(());
            }
        }
        anyhow::bail!("resumed session {short} did not come alive within 10s");
    }

    /// Dispatched-worker bring-up.
    ///
    /// A kube/docker worker pod is a peer machine whose daemon must *start* the
    /// dispatched session itself. The server pre-mints the session id + gateway
    /// token and tells the enrolled dispatcher to spawn the pod, but — unlike a
    /// desktop machine — it sends no WS `Spawn` command, because every
    /// dispatched pod registers under the single shared `dispatch` machine row
    /// and can't be addressed individually. So when the dispatcher-injected env
    /// (`SESSION_ID` + `TASK_PAYLOAD_JSON`) is present, we self-issue the exact
    /// control-socket `dispatch` a server-driven spawn would, reusing
    /// [`Self::prepare_spawn`] and forcing the pre-minted `session_id` so the
    /// gateway token resolves and the registered id matches the dispatch.
    ///
    /// Best-effort: any failure logs and lets the daemon keep observing — it
    /// never aborts `run`. A normal machine daemon lacks these env vars and is
    /// unaffected.
    // Linear startup-dispatch pipeline (read env → build spec → force session_id
    // → spawn) with best-effort bail-out logging at each step; complexity is the
    // env-validation branches, not nesting. Kept whole to preserve the dispatch flow.
    #[allow(clippy::cognitive_complexity)]
    pub(super) async fn maybe_dispatch_on_start(&self) {
        let session_id = match std::env::var("SESSION_ID") {
            Ok(s) if !s.is_empty() => s,
            _ => return,
        };
        let payload_raw = match std::env::var("TASK_PAYLOAD_JSON") {
            Ok(s) if !s.is_empty() => s,
            _ => return,
        };
        let payload: serde_json::Value = match serde_json::from_str(&payload_raw) {
            Ok(v) => v,
            Err(err) => {
                tracing::error!(%err, "dispatch-on-start: TASK_PAYLOAD_JSON is not valid JSON");
                return;
            }
        };
        if already_dispatched(&self.cfg.jobs_root, &session_id) {
            tracing::info!(
                session_id = %session_id,
                "dispatch-on-start: session already dispatched, not re-issuing the prompt"
            );
            return;
        }
        note_dispatched(&self.cfg.jobs_root, &session_id);
        // Codex-native dispatch: a `adapter = "codex"` payload runs
        // headlessly via `codex exec`, NOT the claude control socket. This path
        // is separate from the interactive codex app-server adapter.
        let adapter = crate::dispatch_codex::payload_adapter(&payload);
        if crate::dispatch_codex::is_codex_adapter(&adapter) {
            match crate::dispatch_codex::CodexDispatch::from_payload(&payload) {
                Ok(dispatch) => {
                    tracing::info!(session_id = %session_id, "dispatch-on-start: launching codex dispatch");
                    tokio::spawn(async move {
                        if let Err(err) = dispatch.run().await {
                            tracing::error!(%err, "dispatch-on-start: codex dispatch failed");
                        }
                    });
                }
                Err(err) => {
                    tracing::error!(%err, "dispatch-on-start: could not build codex dispatch");
                }
            }
            return;
        }
        let spec = match Self::build_dispatch_spec(&payload) {
            Ok(spec) => spec,
            Err(err) => {
                tracing::error!(%err, "dispatch-on-start: could not build session spec");
                return;
            }
        };
        let sock = match self.ensure_socket().await {
            Ok(s) => s,
            Err(err) => {
                tracing::error!(%err, "dispatch-on-start: claude daemon socket unavailable");
                return;
            }
        };
        tracing::info!(session_id = %session_id, "dispatch-on-start: launching dispatched session");
        let dispatched = match self.prepare_spawn(&sock, &spec, Some(session_id.clone())).await {
            Ok(dispatch) => dispatch.send().await,
            Err(err) => Err(err),
        };
        if let Err(err) = dispatched {
            tracing::error!(%err, session_id = %session_id, "dispatch-on-start: spawn failed");
        } else {
            tracing::info!(session_id = %session_id, "dispatch-on-start: session dispatched");
            // Arm the turn-complete watcher for this — and only
            // this — session, so the pod entrypoint gets a done-signal when
            // the session settles idle after its work.
            let settle = dispatch_done::settle_from_env(
                std::env::var("CCTUI_DISPATCH_DONE_SETTLE_SECS").ok().as_deref(),
            );
            if let Ok(mut guard) = self.dispatch_done.lock() {
                *guard = Some(DispatchDoneTracker::new(&session_id, &self.cfg.jobs_root, settle));
            }
        }
    }

    /// Build a [`SessionSpec`](cctui_proto::adapter::SessionSpec) from the
    /// dispatcher's `TASK_PAYLOAD_JSON` (`prompt_file`/`prompt`, `model`,
    /// `effort`, `repo`, `env`). Working dir is `CCTUI_DISPATCH_WORKDIR`
    /// (default `/workspace`). Dispatched workers run headless, so the
    /// permission posture is `Yolo` (bypass — the pod is already sandboxed by
    /// landlock + seccomp + the guard-proxy).
    pub(super) fn build_dispatch_spec(
        payload: &serde_json::Value,
    ) -> anyhow::Result<cctui_proto::adapter::SessionSpec> {
        let prompt = Self::resolve_dispatch_prompt(payload)?;
        let workdir =
            std::env::var("CCTUI_DISPATCH_WORKDIR").unwrap_or_else(|_| "/workspace".to_owned());
        let env: std::collections::BTreeMap<String, String> = payload
            .get("env")
            .and_then(serde_json::Value::as_object)
            .map(|o| {
                o.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                    .collect()
            })
            .unwrap_or_default();
        let name = payload
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| std::env::var("TASK_NAME").ok().filter(|s| !s.is_empty()));
        Ok(cctui_proto::adapter::SessionSpec {
            adapter_id: cctui_proto::adapter::AdapterId("claude-code".to_owned()),
            working_dir: Some(workdir),
            prompt: Some(prompt),
            name,
            permission_mode: Some(cctui_proto::adapter::PermissionMode::Yolo),
            effort: payload
                .get("effort")
                .and_then(serde_json::Value::as_str)
                .map(ToOwned::to_owned),
            model: payload.get("model").and_then(serde_json::Value::as_str).map(ToOwned::to_owned),
            service_tier: None,
            env,
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
        })
    }

    /// Resolve the dispatched prompt: an inline `prompt`, else a `prompt_file`
    /// searched across `CCTUI_DISPATCH_PROMPT_DIRS` (default
    /// `/opt/context/prompts:/prompts`). An absolute `prompt_file` is read as-is.
    pub(super) fn resolve_dispatch_prompt(payload: &serde_json::Value) -> anyhow::Result<String> {
        crate::dispatch_codex::resolve_dispatch_prompt(payload)
    }

    /// Spawn a fresh claude session via the `dispatch` op on the `claude
    /// daemon` control socket — the same primitive claude's own `FleetView`
    /// uses (`source:"fleet"`). The new session surfaces in the next `list`
    /// poll and goes through the normal observe path; there is no separate
    /// ACK beyond the dispatch reply.
    ///
    /// The payload shape is the daemon's private, proto-gated dispatch
    /// record; `{op,cwd,prompt}` is rejected as `malformed request`. We mint
    /// the session id / short / nonce client-side exactly as claude does and
    /// hand the worker its launch argv.
    pub(super) async fn prepare_spawn(
        &self,
        sock: &std::path::Path,
        spec: &cctui_proto::adapter::SessionSpec,
        forced_session_id: Option<String>,
    ) -> anyhow::Result<DeferredDispatch> {
        let cwd = require_dir(spec.working_dir.as_deref(), "spawn")?;
        let ids = launch::job_ids(forced_session_id.as_deref());
        let (short, session_id) = (ids.short.as_str(), ids.session_id.as_str());
        self.remember_launch_posture(short, spec);
        // A `CctuiAgent` child links to its caller through the same stash the
        // fork path uses: roster discovery emits the `SessionStarted` and has no
        // other way to know the spawn had a parent. Relation "subagent", not
        // "fork" — the webui nests subagents but renders forks as siblings.
        if let Some(parent) = spec.parent_local_id.as_deref()
            && let Ok(mut map) = self.fork_parent_by_short.lock()
        {
            map.insert(short.to_owned(), (parent.to_owned(), "subagent"));
        }
        let whip = spec.permission_mode.is_some_and(cctui_proto::adapter::PermissionMode::is_whip);
        // Resolved before the managed settings file so the account settings are
        // merged under the managed hooks. Fail-closed: account-bound but
        // unmintable aborts rather than launching a worker that will 401.
        let launch_env = self.resolve_launch_env(session_id, &spec.env).await?;
        let mut launch = spawn_launch(spec, session_id);
        launch.plugin_dirs.clone_from(&launch_env.plugin_dirs);
        launch.mcp_config =
            agent_relay_config(short, session_id, launch_env.spawn_capability.as_ref());
        // The `SessionStart` hook that holds the first turn is only registered
        // when there is a relay to wait for.
        let agent_tool = launch.mcp_config.is_some();
        launch.settings_path = ensure_hook_settings(
            &self.cfg.hook_socket_path,
            whip,
            short,
            launch_env.settings.as_ref(),
            &launch_env.env,
            spec.model.as_deref(),
            spec.effort.as_deref(),
            launch_env.whip_phrases.as_ref(),
            agent_tool.then_some(session_id),
        )
        .map(|p| p.to_string_lossy().into_owned());
        // A staging failure is fatal: silently dropping an attachment the user
        // expects the worker to read would be worse.
        let staged = stage_uploads(session_id, &spec.bootstrap).inspect_err(|_| {
            crate::configsweep::remove_session_files(short);
        })?;
        let session_context = build_session_context(
            spec,
            cwd,
            &staged,
            launch_env.spawn_capability.as_ref().filter(|_| agent_tool),
        );
        let prompt = match spec.prompt.as_deref().map(str::trim) {
            Some(b) if !b.is_empty() => format!("{session_context}\n\n{b}"),
            _ => session_context,
        };
        let req = launch::dispatch_request(
            &ids,
            cwd,
            &launch,
            Some(&prompt),
            &launch_env.env,
            &dispatch_seed(spec),
        );
        let gate = self.launch_gate(session_id, short, spec.model.as_deref());
        Ok(DeferredDispatch {
            sock: sock.to_path_buf(),
            req,
            short: ids.short.clone(),
            what: format!("spawn in {cwd}"),
            session_id: ids.session_id.clone(),
            gate,
        })
    }

    /// Stash the launch model/effort and posture keyed by `short`: the Status
    /// emit falls back to them while `state.json` is still being written (or is
    /// transiently gone across a `/clear`), and the diagnose report reads them.
    pub(super) fn remember_launch_posture(
        &self,
        short: &str,
        spec: &cctui_proto::adapter::SessionSpec,
    ) {
        let model = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
        let effort = spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty());
        if (model.is_some() || effort.is_some())
            && let Ok(mut map) = self.spawn_model_effort.lock()
        {
            map.insert(short.to_owned(), (model.map(str::to_owned), effort.map(str::to_owned)));
        }
        if let Some(mode) = spec.permission_mode
            && let Ok(mut map) = self.spawn_permission_mode.lock()
        {
            map.insert(short.to_owned(), super::super::diagnose::permission_label(mode).to_owned());
        }
    }

    /// The limits gate for a launch, or `None` when no server is configured to
    /// ask — an unattached daemon launches unconditionally.
    pub(super) fn launch_gate(
        &self,
        session_id: &str,
        short: &str,
        model: Option<&str>,
    ) -> Option<LaunchGate> {
        let (server, machine_key) = (self.server.as_ref()?, self.machine_key.as_ref()?);
        Some(LaunchGate {
            server: server.clone(),
            machine_key: machine_key.clone(),
            session_id: session_id.to_owned(),
            short: short.to_owned(),
            model: model.map(str::to_owned),
            events: self.events.clone(),
        })
    }

    /// Fork an existing conversation into a brand-new claude session.
    ///
    /// Mirrors [`spawn`] — mints a fresh `short`/`sessionId`/`nonce` and
    /// dispatches a new worker via the control socket — but prepends `--resume
    /// <parent-session-id> --fork-session` to the launch argv so claude copies
    /// the parent's history into the new session id, leaving the parent intact.
    /// `--model`/`--effort` from `spec` ride on top (this is the supported
    /// "switch model mid-conversation" path).
    ///
    /// The parent session id is resolved to the id claude should resume from:
    /// the parent's on-disk `resumeSessionId` when present (so a `/clear`ed or
    /// `/compact`ed parent forks from the live conversation, not the stale spawn
    /// id), else the parent's `sessionId`, else the `parent_local_id`
    /// itself (covers reopening an archived parent whose `state.json` was removed
    /// by `claude rm` but whose transcript still resumes).
    ///
    /// The child's `SessionStarted` is emitted later by the roster-discovery
    /// path, which has no fork context, so we stash `parent_local_id` keyed by
    /// the new `short` in `fork_parent_by_short` for it to read.
    /// Write a sliced copy of the parent transcript as the child's own
    /// `<child>.jsonl` for a subset fork. Reads the parent JSONL,
    /// keeps only the lines the `extract` selects, and writes the repaired
    /// slice to the child's path under the SAME encoded cwd. Writes are strictly
    /// to the new child file — the parent transcript is never touched.
    pub(super) fn materialize_fork_slice(
        &self,
        cwd: &str,
        parent_session_id: &str,
        child_session_id: &str,
        extract: &cctui_proto::adapter::ForkExtract,
    ) -> anyhow::Result<()> {
        let parent_path =
            transcript::transcript_path(&self.cfg.projects_root, cwd, parent_session_id);
        let raw = std::fs::read_to_string(&parent_path).with_context(|| {
            format!("fork slice: read parent transcript {}", parent_path.display())
        })?;
        let lines: Vec<serde_json::Value> = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .with_context(|| format!("fork slice: parse {}", parent_path.display()))?;
        let kept = super::super::fork_slice::slice_transcript(&lines, extract, child_session_id)?;
        let child_path =
            transcript::transcript_path(&self.cfg.projects_root, cwd, child_session_id);
        if let Some(dir) = child_path.parent() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("fork slice: create child dir {}", dir.display()))?;
        }
        let body: String = kept.iter().map(|l| format!("{l}\n")).collect::<Vec<_>>().concat();
        std::fs::write(&child_path, body).with_context(|| {
            format!("fork slice: write child transcript {}", child_path.display())
        })?;
        tracing::info!(
            parent = %parent_path.display(),
            child = %child_path.display(),
            kept = kept.len(),
            mode = ?extract.mode,
            "materialized subset fork transcript"
        );
        Ok(())
    }

    pub(super) async fn prepare_fork(
        &self,
        sock: &std::path::Path,
        parent_local_id: &str,
        spec: &cctui_proto::adapter::SessionSpec,
        forced_session_id: Option<&str>,
        extract: Option<&cctui_proto::adapter::ForkExtract>,
    ) -> anyhow::Result<DeferredDispatch> {
        let cwd = require_dir(spec.working_dir.as_deref(), "fork")?;

        // Resolve the id to resume+fork from. Prefer the parent's on-disk
        // `resumeSessionId` (the live conversation head after `/clear`/`/compact`),
        // then its `sessionId`, then the raw parent id (archived
        // parent whose job state was removed by `claude rm`, but whose transcript
        // still resumes — the native "reopen archived as a new conversation").
        let resume_id = self
            .resolve_short_for_removal(parent_local_id)
            .ok()
            .and_then(|short| StateJson::read(&self.cfg.jobs_root, &short))
            .and_then(|st| st.resume_session_id.or(st.session_id))
            .unwrap_or_else(|| parent_local_id.to_owned());

        let ids = launch::job_ids(forced_session_id);
        let (short, session_id) = (ids.short.as_str(), ids.session_id.as_str());
        if let Some(extract) = extract {
            self.materialize_fork_slice(cwd, &resume_id, session_id, extract)?;
        }
        let mut launch = fork_launch(spec, &resume_id, session_id, extract.is_some());
        self.remember_launch_posture(short, spec);
        let whip = spec.permission_mode.is_some_and(cctui_proto::adapter::PermissionMode::is_whip);
        // If the server hasn't bound the child id yet, inherit the parent's
        // account env so the child routes through the gateway from its first turn.
        let no_env = std::collections::BTreeMap::default();
        let mut launch_env = self.resolve_launch_env(session_id, &no_env).await?;
        if launch_env.env.is_empty() {
            launch_env = self.resolve_launch_env(parent_local_id, &no_env).await?;
        }
        launch.plugin_dirs.clone_from(&launch_env.plugin_dirs);
        launch.mcp_config =
            agent_relay_config(short, session_id, launch_env.spawn_capability.as_ref());
        launch.settings_path = ensure_hook_settings(
            &self.cfg.hook_socket_path,
            whip,
            short,
            launch_env.settings.as_ref(),
            &launch_env.env,
            None,
            None,
            launch_env.whip_phrases.as_ref(),
            launch.mcp_config.is_some().then_some(session_id),
        )
        .map(|p| p.to_string_lossy().into_owned());
        let prompt = spec.prompt.as_deref().map(str::trim).filter(|p| !p.is_empty());

        // Remember the parent BEFORE dispatching so the roster-discovery emit
        // (which can race in on the very next poll) finds the link.
        if let Ok(mut map) = self.fork_parent_by_short.lock() {
            map.insert(short.to_owned(), (parent_local_id.to_owned(), "fork"));
        }

        let req = launch::dispatch_request(
            &ids,
            cwd,
            &launch,
            prompt,
            &launch_env.env,
            &dispatch_seed(spec),
        );
        tracing::info!(%cwd, %session_id, %parent_local_id, %resume_id, "fork prepared for control socket");
        Ok(DeferredDispatch {
            sock: sock.to_path_buf(),
            req,
            short: ids.short.clone(),
            what: format!("fork of {parent_local_id} in {cwd}"),
            session_id: ids.session_id.clone(),
            gate: None,
        })
    }
}

impl DeferredDispatch {
    /// Send the dispatch and await the daemon's reply. An `ok:false` reply or
    /// a silent daemon (see [`socket::ONE_SHOT_TIMEOUT`]) is an error, and the
    /// worker's managed config files are swept so nothing dangles.
    pub async fn send(self) -> anyhow::Result<()> {
        if let Some(gate) = &self.gate {
            gate.hold().await;
        }
        let resp: serde_json::Value = socket::call(&self.sock, &self.req)
            .await
            .inspect_err(|_| crate::configsweep::remove_session_files(&self.short))
            .with_context(|| format!("dispatch {}", self.what))?;
        tracing::info!(?resp, session_id = %self.session_id, "{} dispatched via control socket", self.what);
        Ok(())
    }

    /// [`send`](Self::send) on its own task, reporting the outcome as the
    /// `CommandResult` for `command_id`.
    pub(super) fn run_detached(
        self,
        events: mpsc::Sender<AdapterEvent>,
        command_id: Option<uuid::Uuid>,
    ) {
        tokio::spawn(async move {
            let res = self.send().await;
            if let Err(err) = &res {
                tracing::warn!(%err, "command dispatch failed");
            }
            Driver::report_command(&events, command_id, res).await;
        });
    }
}

pub(super) fn dispatch_marker_path(jobs_root: &Path, session_id: &str) -> PathBuf {
    jobs_root.join(".cctui-dispatched").join(session_id)
}

/// Whether `session_id` was already dispatched. `SESSION_ID`/`TASK_PAYLOAD_JSON`
/// survive the self-update execve, so without this the whole dispatch prompt is
/// re-sent every five minutes. The `state.json` signal backs up the marker if
/// its directory is wiped; the id is matched in full because an 8-hex job prefix
/// is shared, and a false positive would suppress a first dispatch.
pub(super) fn already_dispatched(jobs_root: &Path, session_id: &str) -> bool {
    if dispatch_marker_path(jobs_root, session_id).exists() {
        return true;
    }
    if session_id.len() < 8 {
        return false;
    }
    StateJson::read(jobs_root, &session_id[..8]).is_some_and(|st| {
        st.session_id.as_deref() == Some(session_id)
            || st.resume_session_id.as_deref() == Some(session_id)
    })
}

/// Record the dispatch before it is issued, not after: a crash between the
/// spawn and its acknowledgement must not buy a second full-context turn.
pub(super) fn note_dispatched(jobs_root: &Path, session_id: &str) {
    let path = dispatch_marker_path(jobs_root, session_id);
    let created = path.parent().map_or(Ok(()), std::fs::create_dir_all);
    if let Err(err) = created.and_then(|()| std::fs::write(&path, b"")) {
        tracing::warn!(%err, path = %path.display(), "dispatch-on-start: cannot write marker");
    }
}

pub(super) fn require_dir<'a>(cwd: Option<&'a str>, what: &str) -> anyhow::Result<&'a str> {
    let cwd = cwd.ok_or_else(|| anyhow::anyhow!("{what}: working_dir required"))?;
    if !std::path::Path::new(cwd).is_dir() {
        anyhow::bail!("{what}: working_dir does not exist or is not a directory: {cwd}");
    }
    Ok(cwd)
}

/// The daemon seeds `name`/`intent` into `state.json` from this; the staged
/// upload paths live in the launch prompt, not the display intent.
pub(super) fn dispatch_seed(spec: &cctui_proto::adapter::SessionSpec) -> serde_json::Value {
    let intent = spec.prompt.clone().or_else(|| spec.name.clone()).unwrap_or_default();
    let mut seed = serde_json::Map::new();
    seed.insert("intent".to_owned(), json!(intent));
    if let Some(name) = &spec.name {
        seed.insert("name".to_owned(), json!(name));
        seed.insert("nameSource".to_owned(), json!("user"));
    }
    serde_json::Value::Object(seed)
}

/// `--session-id <id> --agent claude [--name] [--permission-mode]`, mirroring
/// claude's own fleet dispatch. Model/effort ride the managed `--settings`
/// file instead: the claude daemon applies it to a spare-claimed worker,
/// whereas a `--model` CLI arg forces the cold relaunch that drops the
/// dispatch gateway env.
pub(super) fn spawn_launch(
    spec: &cctui_proto::adapter::SessionSpec,
    session_id: &str,
) -> LaunchArgs {
    LaunchArgs {
        session_id: Some(session_id.to_owned()),
        model: None,
        effort: None,
        ..LaunchArgs::for_dispatch(spec)
    }
}

/// A subset fork's child transcript is a standalone slice, so it resumes
/// itself WITHOUT `--fork-session` (that flag branches off the parent's live
/// history). `--model`/`--effort` ride along: this is the supported "switch
/// model mid-conversation" path.
pub(super) fn fork_launch(
    spec: &cctui_proto::adapter::SessionSpec,
    resume_id: &str,
    session_id: &str,
    sliced: bool,
) -> LaunchArgs {
    LaunchArgs {
        session_id: Some(session_id.to_owned()),
        resume_from: Some(if sliced { session_id } else { resume_id }.to_owned()),
        fork: !sliced,
        ..LaunchArgs::for_dispatch(spec)
    }
}

pub(super) fn resume_launch(session_id: &str) -> LaunchArgs {
    LaunchArgs { resume_from: Some(session_id.to_owned()), ..LaunchArgs::default() }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    #[tokio::test]
    async fn a_detached_dispatch_to_a_dead_daemon_reports_a_failed_command_result() {
        let (tx, mut rx) = mpsc::channel(4);
        let command_id = uuid::Uuid::new_v4();
        deferred(std::env::temp_dir().join(format!("absent-{}.sock", uuid::Uuid::new_v4())))
            .run_detached(tx, Some(command_id));
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("a dead daemon must not hang the command")
            .expect("an outcome");
        match ev {
            AdapterEvent::CommandResult { command_id: got, ok, error } => {
                assert_eq!(got, command_id);
                assert!(!ok);
                assert!(error.is_some_and(|e| e.contains("spawn in /tmp")));
            }
            other => panic!("expected a CommandResult, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_detached_dispatch_reports_ok_once_the_daemon_answers() {
        let dir = std::env::temp_dir().join(format!("cctui-dd-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let sock = dir.join("d.sock");
        let listener = tokio::net::UnixListener::bind(&sock).unwrap();
        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let (r, mut w) = stream.into_split();
            let mut line = String::new();
            tokio::io::AsyncBufReadExt::read_line(&mut tokio::io::BufReader::new(r), &mut line)
                .await
                .unwrap();
            tokio::io::AsyncWriteExt::write_all(&mut w, b"{\"ok\":true}\n").await.unwrap();
        });
        let (tx, mut rx) = mpsc::channel(4);
        let command_id = uuid::Uuid::new_v4();
        deferred(sock).run_detached(tx, Some(command_id));
        let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("an outcome within the bound")
            .expect("an outcome");
        assert!(matches!(ev, AdapterEvent::CommandResult { ok: true, .. }), "{ev:?}");
    }

    #[test]
    fn dispatch_spec_built_from_payload_with_inline_prompt() {
        // the dispatcher injects prompt/model/effort/env inside
        // TASK_PAYLOAD_JSON; the daemon turns it into a headless SessionSpec.
        let payload = serde_json::json!({
            "prompt": "do the thing",
            "model": "opus",
            "effort": "low",
            "repo": "acme",
            "name": "triage-PROJ",
            "env": { "ANTHROPIC_BASE_URL": "https://x/gateway/anthropic", "ANTHROPIC_AUTH_TOKEN": "cctui_s_x" },
        });
        let spec = Driver::build_dispatch_spec(&payload).expect("spec");
        assert_eq!(spec.prompt.as_deref(), Some("do the thing"));
        assert_eq!(spec.model.as_deref(), Some("opus"));
        assert_eq!(spec.effort.as_deref(), Some("low"));
        assert_eq!(spec.name.as_deref(), Some("triage-PROJ"));
        assert_eq!(spec.adapter_id.0, "claude-code");
        assert!(matches!(spec.permission_mode, Some(cctui_proto::adapter::PermissionMode::Yolo)));
        assert_eq!(spec.env.get("ANTHROPIC_AUTH_TOKEN").map(String::as_str), Some("cctui_s_x"));
    }

    #[test]
    fn dispatch_prompt_reads_absolute_file() {
        let dir = std::env::temp_dir().join(format!("cctui-disp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("p.md");
        std::fs::write(&f, "PROMPT BODY").unwrap();
        let payload = serde_json::json!({ "prompt_file": f.to_str().unwrap() });
        assert_eq!(Driver::resolve_dispatch_prompt(&payload).unwrap(), "PROMPT BODY");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dispatch_prompt_errors_when_neither_present() {
        let payload = serde_json::json!({ "model": "opus" });
        assert!(Driver::resolve_dispatch_prompt(&payload).is_err());
    }

    #[test]
    fn a_dispatched_session_is_not_dispatched_again_after_a_reexec() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let session = "6e189420-f9a4-493f-b3d9-e0a80ac254c1";

        assert!(!already_dispatched(root, session), "a fresh pod must dispatch once");
        note_dispatched(root, session);
        assert!(already_dispatched(root, session), "the self-update re-exec must not re-dispatch");
        assert!(!already_dispatched(root, "11111111-2222-3333-4444-555555555555"));
    }

    #[test]
    fn a_live_job_for_the_session_counts_as_dispatched_without_a_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let session = "6e189420-f9a4-493f-b3d9-e0a80ac254c1";
        let job = root.join(&session[..8]);
        std::fs::create_dir_all(&job).unwrap();
        std::fs::write(job.join("state.json"), format!(r#"{{"sessionId":"{session}"}}"#)).unwrap();

        assert!(already_dispatched(root, session));

        let other = r#"{"sessionId":"6e189420-dead-dead-dead-deaddeaddead"}"#;
        std::fs::write(job.join("state.json"), other).unwrap();
        assert!(
            !already_dispatched(root, session),
            "a job merely sharing the 8-hex prefix must not suppress the first dispatch"
        );
    }

    #[test]
    fn the_dispatch_marker_is_not_mistaken_for_a_job_dir() {
        let tmp = tempfile::tempdir().unwrap();
        note_dispatched(tmp.path(), "6e189420-f9a4-493f-b3d9-e0a80ac254c1");
        let dir = dispatch_marker_path(tmp.path(), "x").parent().unwrap().to_owned();
        let name = dir.file_name().unwrap().to_string_lossy().into_owned();
        assert!(crate::configsweep::short_of(&name).is_none(), "must not scan as a job short");
    }

    #[test]
    fn control_launch_argv_snapshot_spawn() {
        let spec = launch_argv_spec();
        let (args, respawn) =
            dispatched_argv(spawn_launch(&spec, "child"), true, Some("ctx\n\ngo"));
        assert_eq!(
            args,
            [
                "--session-id",
                "child",
                "--agent",
                "claude",
                "--name",
                "task",
                "--permission-mode",
                "acceptEdits",
                "--mcp-config",
                "/cfg/mcp.json",
                "--settings",
                "/cfg/settings.json",
                "--",
                "ctx\n\ngo",
            ]
        );
        assert_eq!(
            respawn,
            [
                "--agent",
                "claude",
                "--mcp-config",
                "/cfg/mcp.json",
                "--settings",
                "/cfg/settings.json"
            ]
        );
    }

    #[test]
    fn control_launch_argv_snapshot_fork() {
        let spec = launch_argv_spec();
        let (args, respawn) =
            dispatched_argv(fork_launch(&spec, "parent", "child", false), true, Some("go"));
        assert_eq!(
            args,
            [
                "--resume",
                "parent",
                "--fork-session",
                "--session-id",
                "child",
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
                "--mcp-config",
                "/cfg/mcp.json",
                "--settings",
                "/cfg/settings.json",
                "--",
                "go",
            ]
        );
        let fork_respawn = [
            "--agent",
            "claude",
            "--effort",
            "high",
            "--model",
            "opus",
            "--mcp-config",
            "/cfg/mcp.json",
            "--settings",
            "/cfg/settings.json",
        ];
        assert_eq!(respawn, fork_respawn);
    }

    #[test]
    fn control_launch_argv_snapshot_sliced_fork() {
        let spec = launch_argv_spec();
        let fork_respawn = [
            "--agent",
            "claude",
            "--effort",
            "high",
            "--model",
            "opus",
            "--mcp-config",
            "/cfg/mcp.json",
            "--settings",
            "/cfg/settings.json",
        ];
        let (args, respawn) =
            dispatched_argv(fork_launch(&spec, "parent", "child", true), true, None);
        assert_eq!(
            args,
            [
                "--resume",
                "child",
                "--session-id",
                "child",
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
                "--mcp-config",
                "/cfg/mcp.json",
                "--settings",
                "/cfg/settings.json",
            ]
        );
        assert_eq!(respawn, fork_respawn);
    }

    #[test]
    fn control_launch_argv_snapshot_resume() {
        let (args, respawn) = dispatched_argv(resume_launch("child"), false, None);
        assert_eq!(
            args,
            ["--resume", "child", "--agent", "claude", "--settings", "/cfg/settings.json"]
        );
        assert_eq!(respawn, ["--agent", "claude", "--settings", "/cfg/settings.json"]);
    }

    #[test]
    fn dispatch_seed_names_the_session_when_given() {
        let spec = launch_argv_spec();
        assert_eq!(
            dispatch_seed(&spec),
            json!({"intent": "go", "name": "task", "nameSource": "user"})
        );
        let unnamed = cctui_proto::adapter::SessionSpec { name: None, prompt: None, ..spec };
        assert_eq!(dispatch_seed(&unnamed), json!({"intent": ""}));
    }
}
