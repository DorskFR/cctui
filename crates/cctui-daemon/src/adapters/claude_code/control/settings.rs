use std::fmt::Write as _;

use super::{PathBuf, json};

/// Decode + stage `bootstrap` file uploads under
/// `/tmp/cctui-uploads/<session-id>/`, returning their absolute paths in upload
/// order. Files are written 0600 with sanitized bare names; an empty/null
/// bootstrap yields an empty vec. Errors (bad base64, unwritable dir) abort the
/// spawn so the user learns the attachment didn't land rather than the worker
/// silently starting without it.
/// Build the spawn-time `<session-context>` block prepended to the
/// initial prompt. Mirrors what a human sees on the session card — name,
/// model·effort, permission posture, env var NAMES, cwd, and staged files.
/// Env var VALUES are never included (only `spec.env` keys, sorted by the
/// `BTreeMap`). Empty fields are omitted so the block stays tight.
pub(super) fn build_session_context(
    spec: &cctui_proto::adapter::SessionSpec,
    cwd: &str,
    staged: &[String],
    capability: Option<&cctui_proto::api::SpawnCapability>,
) -> String {
    let mut b = String::from("<session-context>\n");
    if let Some(name) = spec.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
        let _ = writeln!(b, "session: {name}");
    }
    if let Some(model) = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
        match spec.effort.as_deref().map(str::trim).filter(|e| !e.is_empty()) {
            Some(effort) => {
                let _ = writeln!(b, "model: {model} · effort: {effort}");
            }
            None => {
                let _ = writeln!(b, "model: {model}");
            }
        }
    }
    if let Some(mode) = spec.permission_mode {
        let _ = writeln!(b, "permission-mode: {}", mode.normalized_label());
    }
    let _ = writeln!(b, "cwd: {cwd}");
    if !spec.env.is_empty() {
        let names = spec.env.keys().cloned().collect::<Vec<_>>().join(", ");
        let _ = writeln!(b, "env (names only): {names}");
    }
    if !staged.is_empty() {
        b.push_str("attached files:\n");
        for p in staged {
            let _ = writeln!(b, "  - {p}");
        }
    }
    if let Some(cap) = capability.filter(|c| !c.is_empty()) {
        b.push_str(&agent_tool_context(cap));
    }
    b.push_str("</session-context>");
    b
}

const AGENT_TOOL_CONTEXT: &str = include_str!("agent_tool_context.md");

/// The `CctuiAgent` paragraph of the session context: the tool exists, which
/// adapters this session may spawn, and one worked call.
pub(super) fn agent_tool_context(cap: &cctui_proto::api::SpawnCapability) -> String {
    let (intro, usage) = AGENT_TOOL_CONTEXT
        .split_once("{capabilities}\n")
        .expect("agent_tool_context.md has a {capabilities} line");
    let mut b = String::from(intro);
    let _ = writeln!(b, "  adapters you may spawn: {}", cap.adapters.join(", "));
    if let Some(max) = cap.max_budget_usd {
        let _ = writeln!(b, "  per-child budget ceiling: ${max} (inherited when you name none)");
    }
    if let Some(max) = cap.max_children {
        let _ = writeln!(b, "  max children for this session: {max}");
    }
    if let Some(depth) = cap.max_depth {
        let _ = writeln!(b, "  spawn generations left below this session: {depth}");
    }
    let adapter = cap.adapters.first().map_or("claude-code", String::as_str);
    let _ = writeln!(
        b,
        "  example: mcp__cctui__CctuiAgent({{\"adapter\": \"{adapter}\", \"prompt\": \
         \"Review the diff on branch X and list real defects\", \"cwd\": \"/path/to/repo\"}})"
    );
    b.push_str(usage);
    b
}

pub(super) fn stage_uploads(
    session_id: &str,
    bootstrap: &serde_json::Value,
) -> anyhow::Result<Vec<String>> {
    crate::adapters::uploads::stage_bootstrap(session_id, bootstrap)
}

/// Public entry point for mid-chat attachment staging. Thin wrapper
/// over [`crate::adapters::uploads::stage_files`] so the supervisor can stage
/// without reaching into control internals.
pub fn stage_mid_chat_files(
    session_id: &str,
    uploads: &[cctui_proto::adapter::BootstrapFile],
) -> anyhow::Result<Vec<String>> {
    crate::adapters::uploads::stage_files(session_id, uploads)
}

/// Recover a session's whip posture from the per-session settings file the
/// original spawn wrote for `short`. The whip profile is the only one
/// that emits a top-level `hooks.Stop` block (the `whip-stop-hook`), so its
/// presence is a reliable discriminator. Used by cold resume, which has no
/// `spec` to read `permission_mode` from. Absent/unreadable file → not whip.
pub(super) fn detect_whip_from_settings(short: &str) -> bool {
    let Some(path) = hook_settings_path(&format!("hook-settings-{short}.json")) else {
        return false;
    };
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|v| v.get("hooks").and_then(|h| h.get("Stop")).cloned())
        .is_some()
}

pub(super) fn hook_settings_path(file: &str) -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cctui").join(file))
}

/// Write the per-session whip phrase override file the `whip-stop-hook`
/// reads via `--phrases`, returning its path. `None` (unwritable) → the caller
/// launches the hook without the arg, so it uses its compiled defaults.
pub(super) fn write_whip_phrases(short: &str, block: &serde_json::Value) -> Option<PathBuf> {
    let path = hook_settings_path(&format!("whip-phrases-{short}.json"))?;
    if let Some(Err(err)) = path.parent().map(std::fs::create_dir_all) {
        tracing::warn!(%err, "whip-stop: cannot create phrases dir");
        return None;
    }
    match std::fs::write(&path, serde_json::to_vec_pretty(block).ok()?) {
        Ok(()) => Some(path),
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "whip-stop: cannot write phrases");
            None
        }
    }
}

/// Delete a stale whip phrase file for `short` so a spawn after the user cleared
/// the override falls back to the compiled defaults. Best-effort.
pub(super) fn remove_whip_phrases(short: &str) {
    if let Some(path) = hook_settings_path(&format!("whip-phrases-{short}.json")) {
        let _ = std::fs::remove_file(path);
    }
}

/// Recursively deep-merge `overlay` into `base`, with `overlay` winning at every
/// level. Object nodes are merged key-by-key (recursing on shared
/// keys); every other node kind (scalars, arrays) is replaced wholesale by the
/// overlay value. Keys present only in `base` are preserved.
///
/// The daemon uses this to layer its load-bearing managed settings (the ask /
/// permission / Stop hooks) as the `overlay` OVER server-provided per-account
/// settings (the `base`) — so account settings can add keys but can never
/// clobber a managed key. See [`ensure_hook_settings`].
pub(in crate::adapters::claude_code) fn deep_merge(
    base: &mut serde_json::Value,
    overlay: &serde_json::Value,
) {
    match (base, overlay) {
        (serde_json::Value::Object(b), serde_json::Value::Object(o)) => {
            for (k, ov) in o {
                match b.get_mut(k) {
                    Some(bv) => deep_merge(bv, ov),
                    None => {
                        b.insert(k.clone(), ov.clone());
                    }
                }
            }
        }
        (b, o) => *b = o.clone(),
    }
}

/// Produce the final `--settings` document by deep-merging the server-provided
/// per-account `settings` UNDERNEATH the daemon's `managed` settings.
///
/// Managed values win at every level: we start from a clone of the account
/// settings (an object; anything non-object is discarded as malformed) and
/// overlay the managed settings on top via [`deep_merge`]. An account blob that
/// tries to set its own `hooks` therefore loses to the managed `hooks` block —
/// the ask/permission/Stop hooks survive intact.
pub(in crate::adapters::claude_code) fn merge_account_under_managed(
    managed: serde_json::Value,
    account: Option<&serde_json::Value>,
) -> serde_json::Value {
    let mut merged = match account {
        Some(a @ serde_json::Value::Object(_)) => a.clone(),
        // No account settings, or a non-object blob we can't safely merge under:
        // fall back to managed-only, exactly as before.
        _ => return managed,
    };
    deep_merge(&mut merged, &managed);
    merged
}

/// Write (idempotently, on every spawn so it tracks binary upgrades) the
/// managed Claude Code settings file that registers the `AskUserQuestion`
/// PreToolUse/PostToolUse hooks, pointing at this daemon binary and the given
/// delivery socket. Returns the file path to inject via `--settings`,
/// or `None` if we can't locate the binary / config dir (in which case spawning
/// proceeds without the hook rather than failing).
///
/// `whip` toggles the 🐎 enforcement profile: the `AskUserQuestion`
/// `PreToolUse` hook gains `--deny` (it still notifies the UI, but returns a
/// `deny` decision so the form never renders), and a `Stop` hook
/// (`whip-stop-hook`) blocks stalling / hand-back language so the worker runs to
/// genuine completion.
///
/// The file is written to a PER-SESSION path (keyed by `short`) so different
/// sessions — potentially bound to different accounts with different
/// `account_settings` — never clobber each other's `--settings` file.
///
/// `account_settings` is the server-provided, per-account
/// `settings_json` that rode the gateway-env pull. It is deep-merged UNDERNEATH
/// the managed settings: account keys are layered in, but the managed `hooks`
/// block (and any other key the daemon sets) ALWAYS WINS — a malicious or
/// stale account blob that specifies its own `hooks` can never disable the
/// ask/permission/Stop hooks. `None` → managed settings only, exactly as before.
#[allow(clippy::cognitive_complexity, clippy::too_many_arguments)]
pub(in crate::adapters::claude_code) fn ensure_hook_settings(
    sock: &std::path::Path,
    whip: bool,
    short: &str,
    account_settings: Option<&serde_json::Value>,
    gateway_env: &std::collections::BTreeMap<String, String>,
    model: Option<&str>,
    effort: Option<&str>,
    whip_phrases: Option<&serde_json::Value>,
    agent_relay_session: Option<&str>,
) -> Option<PathBuf> {
    let path = hook_settings_path(&format!("hook-settings-{short}.json"))?;
    let exe = std::env::current_exe()
        .map_err(|err| tracing::warn!(%err, "ask-hook: cannot resolve current_exe"))
        .ok()?;
    let exe = exe.to_string_lossy();
    let sock = sock.to_string_lossy();
    let deny = if whip { " --deny" } else { "" };
    let hook = |event: &str| {
        let extra = if event == "pre" { deny } else { "" };
        json!({
            // AskUserQuestion + ExitPlanMode both fire this hook: the former
            // surfaces a live question card, the latter a live Plan card.
            // Both are single-select PTY prompts answered the same
            // way (digit keystroke / dismiss-then-reply).
            "matcher": "AskUserQuestion|ExitPlanMode",
            "hooks": [{
                "type": "command",
                "command": format!("{exe} ask-hook --event {event} --sock {sock}{extra}"),
                "timeout": 5,
            }],
        })
    };
    // Bidirectional tool-permission hook. Scoped to the mutating /
    // executing tools that actually trigger an interactive approval (read-only
    // tools auto-allow, so blocking on them would needlessly stall the turn).
    // Distinct from the `AskUserQuestion` matcher above so both PreToolUse hooks
    // can coexist. The hook BLOCKS, long-polls the daemon for the human's
    // decision, and returns an allow/deny `permissionDecision` — no keystroke in
    // the common case. The `timeout` is deliberately high (the daemon resolves
    // the hook with a `defer` well before this fires, on its own bounded wait);
    // a hook that overran *its* timeout would be treated by Claude Code as a
    // hard deny, which we must never do to a slow human.
    let perm_matcher = "Bash|Edit|MultiEdit|Write|NotebookEdit|WebFetch|Task|KillShell|BashOutput";
    let perm_hook = json!({
        "matcher": perm_matcher,
        "hooks": [{
            "type": "command",
            "command": format!("{exe} ask-hook --event perm --sock {sock}"),
            "timeout": 600,
        }],
    });
    // EnterPlanMode guard. Registered UNCONDITIONALLY of the whip
    // flag: the deny is decided at runtime from the payload's live
    // `permission_mode` (see `enter_plan_mode_decision`), so it must ride the
    // pre event for both Yolo and Whip; `deny` here only sets the posture label.
    let plan_guard = json!({
        "matcher": "EnterPlanMode",
        "hooks": [{
            "type": "command",
            "command": format!("{exe} ask-hook --event pre --sock {sock}{deny}"),
            "timeout": 5,
        }],
    });
    let pre_hooks = json!([hook("pre"), perm_hook, plan_guard]);
    // The whip Stop hook gets the user's phrase override via a
    // per-session file it reads with `--phrases`; absent/cleared → the hook falls
    // back to its compiled defaults, so a stale file from a prior spawn is removed.
    let whip_stop_command = if whip {
        let arg = whip_phrases.filter(|v| !v.is_null()).map_or_else(
            || {
                remove_whip_phrases(short);
                String::new()
            },
            |v| {
                write_whip_phrases(short, v)
                    .map(|p| format!(" --phrases {}", p.to_string_lossy()))
                    .unwrap_or_default()
            },
        );
        format!("{exe} whip-stop-hook{arg}")
    } else {
        String::new()
    };
    let mut hooks = if whip {
        json!({
            "PreToolUse": pre_hooks,
            "PostToolUse": [hook("post")],
            "Stop": [{
                "hooks": [{
                    "type": "command",
                    "command": whip_stop_command,
                    "timeout": 10,
                }],
            }],
        })
    } else {
        json!({ "PreToolUse": pre_hooks, "PostToolUse": [hook("post")] })
    };
    // Claude Code connects its MCP servers while the session starts, so a turn-1
    // `CctuiAgent` call can beat the relay's `initialize`.
    if let Some(block) = agent_relay_session.and_then(|session| {
        let sock = crate::agenttool::socket_for_launch().to_string_lossy().into_owned();
        mcp_ready_hook(&exe, session, &sock, mcp_ready_wait_secs())
    }) {
        hooks["SessionStart"] = block;
    }
    let managed = managed_settings(hooks, gateway_env, model, effort);
    // Layer the server-provided per-account settings UNDERNEATH the managed
    // settings: account keys are merged in, but the managed keys
    // (hooks, gateway env, model/effort) always win so they can never be
    // clobbered.
    let settings = merge_account_under_managed(managed, account_settings);
    if let Some(Err(err)) = path.parent().map(std::fs::create_dir_all) {
        tracing::warn!(%err, "ask-hook: cannot create settings dir");
        return None;
    }
    if let Err(err) = std::fs::write(&path, serde_json::to_vec_pretty(&settings).ok()?) {
        tracing::warn!(%err, path = %path.display(), "ask-hook: cannot write settings");
        return None;
    }
    // The file now carries the gateway bearer token — restrict it to
    // owner-only so the secret isn't world-readable on disk.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)) {
            tracing::warn!(%err, path = %path.display(), "ask-hook: cannot chmod settings 0600");
        }
    }
    Some(path)
}

/// The `SessionStart` block that holds the first turn until `session`'s MCP
/// relay is up. `None` for a zero wait, which disables the gate.
///
/// The hook's own timeout is the wait plus a margin: a hook that overruns its
/// timeout is treated by Claude Code as a failure, so the wait must always be
/// the thing that expires first.
pub(super) fn mcp_ready_hook(
    exe: &str,
    session: &str,
    sock: &str,
    wait_secs: u64,
) -> Option<serde_json::Value> {
    (wait_secs > 0).then(|| {
        json!([{
            "hooks": [{
                "type": "command",
                "command": format!(
                    "{exe} mcp-wait --session {session} --sock {sock} --timeout {wait_secs}"
                ),
                "timeout": wait_secs + 5,
            }],
        }])
    })
}

/// Seconds the `SessionStart` hook may hold the first turn waiting for the MCP
/// relay. `CCTUI_MCP_READY_WAIT_SECS=0` disables the gate entirely.
pub(super) fn mcp_ready_wait_secs() -> u64 {
    std::env::var("CCTUI_MCP_READY_WAIT_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(8)
        .min(60)
}

/// Write the per-session MCP config registering the `CctuiAgent` tool, and
/// return the path to inject as `--mcp-config`.
///
/// `None` — no tool — whenever the session has no spawn capability, so a session
/// the server never granted spawn rights cannot even see the tool. The config is
/// keyed by `short` like the hook settings so sessions never clobber each other.
pub(in crate::adapters::claude_code) fn ensure_agent_mcp_config(
    short: &str,
    session_id: &str,
    capability: Option<&cctui_proto::api::SpawnCapability>,
) -> Option<PathBuf> {
    if capability.is_none_or(cctui_proto::api::SpawnCapability::is_empty) {
        return None;
    }
    let path = hook_settings_path(&format!("mcp-agent-{short}.json"))?;
    let exe = std::env::current_exe()
        .map_err(|err| tracing::warn!(%err, "CctuiAgent: cannot resolve current_exe"))
        .ok()?;
    let config = crate::mcp::mcp_config(
        &exe.to_string_lossy(),
        session_id,
        crate::agenttool::socket_for_launch(),
    );
    if let Some(Err(err)) = path.parent().map(std::fs::create_dir_all) {
        tracing::warn!(%err, "CctuiAgent: cannot create mcp config dir");
        return None;
    }
    if let Err(err) = std::fs::write(&path, serde_json::to_vec_pretty(&config).ok()?) {
        tracing::warn!(%err, path = %path.display(), "CctuiAgent: cannot write mcp config");
        return None;
    }
    Some(path)
}

/// The agent relay `--mcp-config` for a worker launch, or `None` when the
/// session has no spawn capability.
pub(super) fn agent_relay_config(
    short: &str,
    session_id: &str,
    capability: Option<&cctui_proto::api::SpawnCapability>,
) -> Option<String> {
    let mcp = ensure_agent_mcp_config(short, session_id, capability)?;
    crate::mcpready::note_launch(session_id);
    Some(mcp.to_string_lossy().into_owned())
}

/// Build the managed `--settings` document: the ask/permission/Stop
/// `hooks`, the gateway routing `env`, and the session `model`/`effortLevel`,
/// all in one file. The claude daemon applies a session's `--settings` to a
/// spare-claimed worker but deliberately does NOT reapply the dispatch `env`, so
/// carrying the gateway env HERE is the only channel that survives the
/// spare-claim on every platform (replacing the Linux-`/proc`-only gateway-env
/// heal). Split out from [`ensure_hook_settings`] so the shape is unit-testable
/// without touching the filesystem.
pub(super) fn managed_settings(
    hooks: serde_json::Value,
    gateway_env: &std::collections::BTreeMap<String, String>,
    model: Option<&str>,
    effort: Option<&str>,
) -> serde_json::Value {
    let mut managed = serde_json::Map::new();
    managed.insert("hooks".to_owned(), hooks);
    if let Some(m) = model.map(str::trim).filter(|m| !m.is_empty()) {
        managed.insert("model".to_owned(), json!(m));
    }
    let mut env_obj: serde_json::Map<String, serde_json::Value> =
        gateway_env.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    // `effortLevel` only accepts low|medium|high|xhigh; `max`/`ultracode` are
    // session-only and rejected in a settings file, so they ride the
    // `CLAUDE_CODE_EFFORT_LEVEL` env var instead (which accepts them).
    if let Some(e) = effort.map(str::trim).filter(|e| !e.is_empty()) {
        if matches!(e, "low" | "medium" | "high" | "xhigh") {
            managed.insert("effortLevel".to_owned(), json!(e));
        } else {
            env_obj.insert("CLAUDE_CODE_EFFORT_LEVEL".to_owned(), json!(e));
        }
    }
    if !env_obj.is_empty() {
        managed.insert("env".to_owned(), serde_json::Value::Object(env_obj));
    }
    serde_json::Value::Object(managed)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    #[test]
    fn no_capability_means_no_mcp_config_and_so_no_tool() {
        assert!(ensure_agent_mcp_config("aaaaaaa1", "sess-1", None).is_none());
        let empty = cctui_proto::api::SpawnCapability::default();
        assert!(
            ensure_agent_mcp_config("aaaaaaa1", "sess-1", Some(&empty)).is_none(),
            "an empty adapter list grants nothing, so the tool must not be registered"
        );
    }

    #[test]
    fn a_capability_writes_a_session_scoped_mcp_config() {
        let cap = cctui_proto::api::SpawnCapability {
            adapters: vec!["opencode".to_owned()],
            max_budget_usd: Some(1.0),
            max_children: Some(2),
            ..Default::default()
        };
        let short = format!("{:08x}", std::process::id());
        let Some(path) = ensure_agent_mcp_config(&short, "sess-42", Some(&cap)) else {
            return; // no writable config dir in this environment
        };
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let args = written["mcpServers"]["cctui"]["args"].as_array().unwrap().clone();
        assert!(args.contains(&json!("mcp-agent")));
        assert!(args.contains(&json!("sess-42")), "the session id is fixed in argv");
        assert!(path.to_string_lossy().contains(&short), "config must be per-session");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn agent_relay_config_is_session_scoped_and_needs_a_capability() {
        let cap = cctui_proto::api::SpawnCapability {
            adapters: vec!["claude-code".to_owned()],
            max_budget_usd: Some(1.0),
            max_children: Some(2),
            ..Default::default()
        };
        let short = format!("{:08x}", std::process::id() ^ 0x5eed);
        assert!(agent_relay_config(&short, "sess-1", None).is_none());
        let Some(path) = agent_relay_config(&short, "sess-1", Some(&cap)) else {
            return; // no writable config dir in this environment
        };
        assert!(path.contains(&short));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn managed_settings_carries_gateway_env_model_and_effort() {
        // gateway env + model + effort ride the `--settings` file so
        // they survive the claude-daemon spare-claim (which drops the dispatch
        // env). Enum efforts use the `effortLevel` key; `max` falls back to the
        // CLAUDE_CODE_EFFORT_LEVEL env var (the settings key rejects it).
        let mut env = std::collections::BTreeMap::new();
        env.insert("ANTHROPIC_BASE_URL".to_owned(), "https://x/gateway/anthropic".to_owned());
        env.insert("ANTHROPIC_AUTH_TOKEN".to_owned(), "cctui_s_tok".to_owned());

        let s = managed_settings(
            json!({ "PreToolUse": [] }),
            &env,
            Some("claude-fable-5[1m]"),
            Some("medium"),
        );
        assert_eq!(s["model"], json!("claude-fable-5[1m]"));
        assert_eq!(s["effortLevel"], json!("medium"));
        assert_eq!(s["env"]["ANTHROPIC_BASE_URL"], json!("https://x/gateway/anthropic"));
        assert_eq!(s["env"]["ANTHROPIC_AUTH_TOKEN"], json!("cctui_s_tok"));
        assert!(s["hooks"].is_object());

        // `max` is not a valid `effortLevel`; it rides the env var instead.
        let s = managed_settings(json!({}), &env, None, Some("max"));
        assert!(s.get("effortLevel").is_none());
        assert_eq!(s["env"]["CLAUDE_CODE_EFFORT_LEVEL"], json!("max"));

        // No env / model / effort → only hooks, no empty `env` object.
        let s = managed_settings(json!({}), &std::collections::BTreeMap::new(), None, None);
        assert!(s.get("env").is_none());
        assert!(s.get("model").is_none());
    }

    #[test]
    fn deep_merge_overlay_wins_and_recurses() {
        // overlay wins at every level; base-only keys are preserved;
        // nested objects merge key-by-key rather than replacing wholesale.
        let mut base = json!({
            "a": 1,
            "keep": "me",
            "nested": { "x": "base-x", "base_only": true },
        });
        let overlay = json!({
            "a": 2,
            "nested": { "x": "overlay-x", "y": "overlay-y" },
        });
        deep_merge(&mut base, &overlay);
        assert_eq!(base["a"], json!(2), "overlay scalar wins");
        assert_eq!(base["keep"], json!("me"), "base-only top key preserved");
        assert_eq!(base["nested"]["x"], json!("overlay-x"), "overlay wins nested");
        assert_eq!(base["nested"]["base_only"], json!(true), "base-only nested key preserved");
        assert_eq!(base["nested"]["y"], json!("overlay-y"), "overlay-only nested key added");
    }

    #[test]
    fn account_settings_cannot_clobber_managed_hooks() {
        // (a) An account blob that specifies its OWN hooks must lose to the
        // managed hooks entirely — the ask/permission hooks survive intact.
        let managed = managed_ask_settings();
        let account = json!({
            "hooks": { "PreToolUse": [{ "matcher": "*", "hooks": [{ "type": "command", "command": "evil --disable" }] }] },
            "env": { "MY_ACCOUNT_VAR": "1" },
            "permissions": { "allow": ["Bash(ls:*)"] },
        });
        let merged = merge_account_under_managed(managed.clone(), Some(&account));
        // Managed hooks win wholesale (the malicious PreToolUse never appears).
        assert_eq!(merged["hooks"], managed["hooks"], "managed hooks always win");
        assert_eq!(
            merged["hooks"]["PreToolUse"].as_array().unwrap().len(),
            2,
            "both managed PreToolUse hooks (ask + perm) survive"
        );
        // (b) account NON-hook keys are merged in.
        assert_eq!(merged["env"]["MY_ACCOUNT_VAR"], json!("1"));
        assert_eq!(merged["permissions"]["allow"], json!(["Bash(ls:*)"]));
    }

    #[test]
    fn account_non_hook_keys_merge_and_nested_managed_survives() {
        // (c) A nested merge where the account also supplies `hooks.PostToolUse`
        // must not drop the managed `hooks.PreToolUse` sub-key.
        let managed = managed_ask_settings();
        let account = json!({
            "hooks": { "PostToolUse": [{ "matcher": "*", "hooks": [{ "command": "acct-post" }] }], "SessionStart": [{ "hooks": [] }] },
            "statusLine": { "type": "command", "command": "mystatus" },
        });
        let merged = merge_account_under_managed(managed.clone(), Some(&account));
        // Managed PreToolUse + PostToolUse both intact (managed wins on PostToolUse).
        assert_eq!(merged["hooks"]["PreToolUse"], managed["hooks"]["PreToolUse"]);
        assert_eq!(merged["hooks"]["PostToolUse"], managed["hooks"]["PostToolUse"]);
        // Account's brand-new hook event (no managed counterpart) is added.
        assert!(merged["hooks"]["SessionStart"].is_array());
        // Non-hook top-level account key merged in.
        assert_eq!(merged["statusLine"]["command"], json!("mystatus"));
    }

    #[test]
    fn account_env_block_reaches_settings_but_managed_env_wins() {
        // curated env vars persist in the account `settings_json.env`
        // block. That block must survive the merge under managed settings so it
        // reaches the worker's process env — but a managed gateway env key of the
        // same name always wins (routing can never be clobbered).
        let managed = managed_settings(
            managed_ask_settings()["hooks"].clone(),
            &env_of(&[("ANTHROPIC_BASE_URL", "https://x/gateway/anthropic")]),
            None,
            None,
        );
        let account = json!({
            "env": { "DISABLE_TELEMETRY": "1", "ANTHROPIC_BASE_URL": "https://evil" },
        });
        let merged = merge_account_under_managed(managed, Some(&account));
        // Account's own curated env var survives.
        assert_eq!(merged["env"]["DISABLE_TELEMETRY"], json!("1"));
        // Managed gateway env wins over an account attempt to override it.
        assert_eq!(merged["env"]["ANTHROPIC_BASE_URL"], json!("https://x/gateway/anthropic"));
    }

    #[test]
    fn no_account_settings_is_managed_only() {
        let managed = managed_ask_settings();
        assert_eq!(merge_account_under_managed(managed.clone(), None), managed);
        // A non-object account blob is treated as absent (never merged).
        assert_eq!(merge_account_under_managed(managed.clone(), Some(&json!("garbage"))), managed);
    }

    #[test]
    fn stage_uploads_writes_sanitized_0600_files() {
        use base64::Engine;
        use std::os::unix::fs::PermissionsExt;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
        // A normal name and a traversal attempt that must collapse to its basename.
        let bootstrap = json!({
            "uploads": [
                { "name": "notes.txt", "content_b64": b64("hello world") },
                { "name": "../../etc/evil", "content_b64": b64("nope") },
            ]
        });

        let paths = stage_uploads(&session_id, &bootstrap).expect("stage ok");
        assert_eq!(paths.len(), 2);
        let dir = std::path::Path::new("/tmp/cctui-uploads").join(&session_id);

        let notes = dir.join("notes.txt");
        assert!(paths.contains(&notes.to_string_lossy().into_owned()));
        assert_eq!(std::fs::read_to_string(&notes).unwrap(), "hello world");
        let mode = std::fs::metadata(&notes).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "uploaded file must be 0600");

        // Traversal collapsed to the bare basename inside the staging dir.
        let evil = dir.join("evil");
        assert!(evil.exists(), "traversal name must be reduced to a basename in-dir");
        assert!(!std::path::Path::new("/tmp/cctui-uploads").join("../../etc/evil").exists());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stage_uploads_null_bootstrap_is_empty() {
        assert!(stage_uploads("sid", &serde_json::Value::Null).unwrap().is_empty());
    }

    #[test]
    fn session_context_block_lists_env_names_not_values() {
        use cctui_proto::adapter::{AdapterId, PermissionMode, SessionSpec};
        let mut env = std::collections::BTreeMap::new();
        env.insert("CCTUI_GITHUB_TOKEN".to_owned(), "super-secret".to_owned());
        env.insert("REGISTRY_USER".to_owned(), "admin".to_owned());
        let spec = SessionSpec {
            service_tier: None,
            adapter_id: AdapterId::new("claude-code"),
            working_dir: Some("/work/cctui".to_owned()),
            prompt: Some("refactor it".to_owned()),
            name: Some("refactor the dispatcher".to_owned()),
            permission_mode: Some(PermissionMode::Auto),
            effort: Some("high".to_owned()),
            model: Some("opus".to_owned()),
            env,
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
        };
        let block = build_session_context(&spec, "/work/cctui", &["a.rs".to_owned()], None);
        assert!(block.starts_with("<session-context>\n"));
        assert!(block.ends_with("</session-context>"));
        assert!(block.contains("session: refactor the dispatcher"));
        assert!(block.contains("model: opus · effort: high"));
        // acceptEdits/Auto normalizes to `auto`.
        assert!(block.contains("permission-mode: auto"));
        assert!(block.contains("cwd: /work/cctui"));
        assert!(block.contains("env (names only): CCTUI_GITHUB_TOKEN, REGISTRY_USER"));
        assert!(block.contains("  - a.rs"));
        // VALUES must never leak.
        assert!(!block.contains("super-secret"));
        assert!(!block.contains("admin"));
        assert!(!block.contains("CctuiAgent"));
    }

    #[test]
    fn session_context_advertises_the_agent_tool_when_capable() {
        use cctui_proto::adapter::{AdapterId, SessionSpec};
        let spec = SessionSpec {
            service_tier: None,
            adapter_id: AdapterId::new("claude-code"),
            working_dir: Some("/work/cctui".to_owned()),
            prompt: None,
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            env: std::collections::BTreeMap::new(),
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
        };
        let cap = cctui_proto::api::SpawnCapability::machine_default();
        let block = build_session_context(&spec, "/work/cctui", &[], Some(&cap));
        assert!(block.contains("mcp__cctui__CctuiAgent"));
        assert!(block.contains("adapters you may spawn: claude-code, codex, opencode"));
        assert!(block.contains("per-child budget ceiling: $20"));
        assert!(block.contains("example: mcp__cctui__CctuiAgent({\"adapter\": \"claude-code\""));
        assert!(block.contains("mcp__cctui__CctuiUsage"), "the limits tool is announced too");
        assert!(block.contains("blocked model burns the whole batch"), "{block}");
        assert!(
            block.contains("retry the call once"),
            "turn 1 can beat the relay, so the model is told to retry: {block}"
        );
        assert!(block.ends_with("</session-context>"));

        let empty = cctui_proto::api::SpawnCapability::default();
        let block = build_session_context(&spec, "/work/cctui", &[], Some(&empty));
        assert!(!block.contains("CctuiAgent"), "an empty capability advertises nothing");
        assert!(!block.contains("CctuiUsage"), "the relay is absent, so neither tool exists");
    }

    #[test]
    fn the_session_start_hook_waits_for_the_relay_and_outlives_its_own_wait() {
        let block = mcp_ready_hook("/usr/bin/cctui-daemon", "sess-1", "/run/a.sock", 8)
            .expect("a positive wait registers the gate");
        let hook = &block[0]["hooks"][0];
        assert_eq!(
            hook["command"],
            "/usr/bin/cctui-daemon mcp-wait --session sess-1 --sock /run/a.sock --timeout 8"
        );
        assert_eq!(
            hook["timeout"], 13,
            "the hook timeout must exceed the wait, or Claude Code kills it as a failure"
        );
        assert!(
            mcp_ready_hook("/usr/bin/cctui-daemon", "sess-1", "/run/a.sock", 0).is_none(),
            "a zero wait disables the gate"
        );
    }

    #[test]
    fn stage_mid_chat_files_suffixes_name_collisions() {
        use base64::Engine;
        use cctui_proto::adapter::BootstrapFile;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
        let dir = std::path::Path::new("/tmp/cctui-uploads").join(&session_id);

        // First upload stages report.pdf.
        let first = stage_mid_chat_files(
            &session_id,
            &[BootstrapFile { name: "report.pdf".into(), content_b64: b64("one") }],
        )
        .expect("stage ok");
        assert_eq!(first, vec![dir.join("report.pdf").to_string_lossy().into_owned()]);

        // A later upload with the same name must NOT overwrite — it gets a suffix.
        let second = stage_mid_chat_files(
            &session_id,
            &[
                BootstrapFile { name: "report.pdf".into(), content_b64: b64("two") },
                BootstrapFile { name: "report.pdf".into(), content_b64: b64("three") },
            ],
        )
        .expect("stage ok");
        assert_eq!(
            second,
            vec![
                dir.join("report-1.pdf").to_string_lossy().into_owned(),
                dir.join("report-2.pdf").to_string_lossy().into_owned(),
            ]
        );
        assert_eq!(std::fs::read_to_string(dir.join("report.pdf")).unwrap(), "one");
        assert_eq!(std::fs::read_to_string(dir.join("report-1.pdf")).unwrap(), "two");
        assert_eq!(std::fs::read_to_string(dir.join("report-2.pdf")).unwrap(), "three");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_tool_context_renders_byte_identical_text() {
        let full = cctui_proto::api::SpawnCapability {
            adapters: vec!["codex".to_owned(), "claude-code".to_owned()],
            max_budget_usd: Some(2.5),
            max_children: Some(3),
            max_depth: Some(1),
            ..Default::default()
        };
        assert_eq!(
            agent_tool_context(&full),
            "CctuiAgent: you can delegate work to cctui subagent sessions on this machine with \
             the MCP tool `mcp__cctui__CctuiAgent`. Each child is a real cctui session — nested \
             under this one in the UI, metered, killable. The call follows the child (progress \
             streams back while it works) and returns its final message when its turn completes. \
             Parallel calls run in parallel. To send a follow-up to a child, call again with its \
             session_id (included in the reply) and a new prompt.\n  adapters you may spawn: \
             codex, claude-code\n  per-child budget ceiling: $2.5 (inherited when you name \
             none)\n  max children for this session: 3\n  spawn generations left below this \
             session: 1\n  example: mcp__cctui__CctuiAgent({\"adapter\": \"codex\", \"prompt\": \
             \"Review the diff on branch X and list real defects\", \"cwd\": \
             \"/path/to/repo\"})\nCctuiUsage: `mcp__cctui__CctuiUsage` reports the rate limits \
             and budget that apply to THIS session — the account it is pinned to (possibly \
             shared or pool-elected, not necessarily your own), its usage windows, this \
             session's spend, and whether each model is currently allowed or soft-limit blocked. \
             Check it before a fan-out and when picking a child's model: a blocked model burns \
             the whole batch on 429s. Takes no arguments.\nBoth tools are served by an MCP \
             server that connects as this session starts. If either reports \"No such tool \
             available\" on your first turn, it lost that race: wait a few seconds and retry the \
             call once before concluding the tool is missing.\n"
        );
        assert_eq!(
            agent_tool_context(&cctui_proto::api::SpawnCapability::default()),
            "CctuiAgent: you can delegate work to cctui subagent sessions on this machine with \
             the MCP tool `mcp__cctui__CctuiAgent`. Each child is a real cctui session — nested \
             under this one in the UI, metered, killable. The call follows the child (progress \
             streams back while it works) and returns its final message when its turn completes. \
             Parallel calls run in parallel. To send a follow-up to a child, call again with its \
             session_id (included in the reply) and a new prompt.\n  adapters you may spawn: \n  \
             example: mcp__cctui__CctuiAgent({\"adapter\": \"claude-code\", \"prompt\": \"Review \
             the diff on branch X and list real defects\", \"cwd\": \
             \"/path/to/repo\"})\nCctuiUsage: `mcp__cctui__CctuiUsage` reports the rate limits \
             and budget that apply to THIS session — the account it is pinned to (possibly \
             shared or pool-elected, not necessarily your own), its usage windows, this \
             session's spend, and whether each model is currently allowed or soft-limit blocked. \
             Check it before a fan-out and when picking a child's model: a blocked model burns \
             the whole batch on 429s. Takes no arguments.\nBoth tools are served by an MCP \
             server that connects as this session starts. If either reports \"No such tool \
             available\" on your first turn, it lost that race: wait a few seconds and retry the \
             call once before concluding the tool is missing.\n"
        );
    }
}
