use std::fmt::Write as _;
use std::sync::LazyLock;

use cctui_proto::hook_policy::HookPolicy;
use cctui_proto::injected::SESSION_CONTEXT;

use crate::hook_policy::HookPolicySink as _;
use crate::hook_policy::claude_code::ClaudeCode;

use super::{PathBuf, json};

const REPLY_LINE: &str = "replies: your thinking is never shown to the user as a reply; anything \
    meant for them (answers, image markers) must be in your message text.\n";

/// Build the spawn-time `<cctui-session-context>` block prepended to the
/// initial prompt. Mirrors what a human sees on the session card — name,
/// model·effort, permission posture, env var NAMES, cwd, and staged files.
/// Env var VALUES are never included (only `spec.env` keys, sorted by the
/// `BTreeMap`). Empty fields are omitted so the block stays tight.
pub(super) fn build_session_context(
    spec: &cctui_proto::adapter::SessionSpec,
    cwd: &str,
    staged: &[String],
    capability: Option<&cctui_proto::api::SpawnCapability>,
    context: Option<&str>,
) -> String {
    let mut b = format!("<{SESSION_CONTEXT}>\n");
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
    if let Some(context) = context.map(str::trim).filter(|c| !c.is_empty()) {
        let _ = writeln!(b, "{context}");
    }
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
    b.push_str(REPLY_LINE);
    b.push_str(crate::preamble::IMAGE_LINE);
    if let Some(cap) = capability.filter(|c| !c.is_empty()) {
        b.push_str(&agent_tool_context(cap));
    }
    let _ = write!(b, "</{SESSION_CONTEXT}>");
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
    match cap.max_budget_usd {
        Some(max) => {
            let _ =
                writeln!(b, "  per-child budget ceiling: ${max} (inherited when you name none)");
        }
        None => {
            b.push_str("  no budget limit: children run uncapped unless you name a budget_usd\n");
        }
    }
    match cap.max_children {
        Some(max) => {
            let _ = writeln!(b, "  max children for this session: {max}");
        }
        None => {
            b.push_str("  no limit on the number of children\n");
        }
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
/// original spawn wrote for `short`. Every session registers a `hooks.Stop`
/// block, so the discriminator is the `whip-stop-hook` command inside it. Used
/// by cold resume, which has no `spec` to read `permission_mode` from.
/// Absent/unreadable file → not whip.
pub(super) fn detect_whip_from_settings(short: &str) -> bool {
    let Some(path) = hook_settings_path(&format!("hook-settings-{short}.json")) else {
        return false;
    };
    std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .is_some_and(|v| stop_block_is_whip(v.pointer("/hooks/Stop")))
}

fn stop_block_is_whip(stop: Option<&serde_json::Value>) -> bool {
    stop.and_then(|s| serde_json::to_string(s).ok()).is_some_and(|s| s.contains("whip-stop-hook"))
}

/// The directory holding the per-session settings, mcp-config and whip-phrase
/// files. The `*_in` writers take it as an argument because mutating the process
/// env is unsound under the threaded test runner.
pub(super) fn config_root() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("cctui"))
}

/// A native respawn changes `source` away from cctui. The private settings
/// file's session binding is the durable evidence that this remains our job.
pub(super) fn is_managed_gateway_settings(path: &std::path::Path, session_id: &str) -> bool {
    if !std::fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink()) {
        return false;
    }
    let Ok(bytes) = std::fs::read(path) else { return false };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else { return false };
    value["env"]["CCTUI_SESSION_ID"].as_str() == Some(session_id)
        && value["env"]["ANTHROPIC_AUTH_TOKEN"].as_str().is_some_and(|t| t.starts_with("cctui_s_"))
        && value["env"]["ANTHROPIC_BASE_URL"].as_str().is_some_and(|u| !u.trim().is_empty())
}

pub(super) fn hook_settings_path(file: &str) -> Option<PathBuf> {
    Some(config_root()?.join(file))
}

/// Write the per-session whip phrase override file the `whip-stop-hook`
/// reads via `--phrases`, returning its path. `None` (unwritable) → the caller
/// launches the hook without the arg, so it uses its compiled defaults.
fn write_whip_phrases_in(
    root: &std::path::Path,
    short: &str,
    block: &serde_json::Value,
) -> Option<PathBuf> {
    let path = root.join(format!("whip-phrases-{short}.json"));
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
fn remove_whip_phrases_in(root: &std::path::Path, short: &str) {
    let _ = std::fs::remove_file(root.join(format!("whip-phrases-{short}.json")));
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

/// The hook policy a launch applies, and the daemon secret that seals the copy
/// kept for a cold resume.
#[derive(Clone, Copy, Default)]
pub(in crate::adapters::claude_code) struct LaunchPolicy<'a> {
    pub policy: Option<&'a HookPolicy>,
    pub seal_key: Option<&'a str>,
}

/// Write afresh, on every launch, resume and turn, the
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
/// ask/permission/Stop hooks. `None` → managed settings only.
///
/// `hook_policy` rules are appended after cctui's own hooks and kept for a
/// cold resume, which has no spawn spec: see [`recover_hook_policy`].
#[allow(clippy::too_many_arguments)]
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
    hook_policy: LaunchPolicy<'_>,
) -> Option<PathBuf> {
    write_hook_settings_in(
        &config_root()?,
        sock,
        whip,
        short,
        account_settings,
        gateway_env,
        model,
        effort,
        whip_phrases,
        agent_relay_session,
        hook_policy,
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn ensure_hook_settings_in(
    root: &std::path::Path,
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
    write_hook_settings_in(
        root,
        sock,
        whip,
        short,
        account_settings,
        gateway_env,
        model,
        effort,
        whip_phrases,
        agent_relay_session,
        LaunchPolicy::default(),
    )
}

#[allow(clippy::cognitive_complexity, clippy::too_many_arguments)]
fn write_hook_settings_in(
    root: &std::path::Path,
    sock: &std::path::Path,
    whip: bool,
    short: &str,
    account_settings: Option<&serde_json::Value>,
    gateway_env: &std::collections::BTreeMap<String, String>,
    model: Option<&str>,
    effort: Option<&str>,
    whip_phrases: Option<&serde_json::Value>,
    agent_relay_session: Option<&str>,
    hook_policy: LaunchPolicy<'_>,
) -> Option<PathBuf> {
    let path = root.join(format!("hook-settings-{short}.json"));
    let exe = std::env::current_exe()
        .map_err(|err| tracing::warn!(%err, "ask-hook: cannot resolve current_exe"))
        .ok()?;
    let exe = exe.to_string_lossy();
    let sock = sock.to_string_lossy();
    let hook = |event: &str| {
        json!({
            // AskUserQuestion + ExitPlanMode both fire this hook: the former
            // surfaces a live question card, the latter a live Plan card.
            // Both are single-select PTY prompts answered the same
            // way (digit keystroke / dismiss-then-reply).
            "matcher": "AskUserQuestion|ExitPlanMode",
            "hooks": [{
                "type": "command",
                "command": ask_hook_command(&exe, event, &sock, whip && event == "pre"),
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
            "command": ask_hook_command(&exe, "perm", &sock, false),
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
            "command": ask_hook_command(&exe, "pre", &sock, whip),
            "timeout": 5,
        }],
    });
    let worktree_guard = json!({
        "matcher": "Agent|Task",
        "hooks": [{
            "type": "command",
            "command": ask_hook_command(&exe, "pre", &sock, false),
            "timeout": 5,
        }],
    });
    let pre_hooks = json!([hook("pre"), perm_hook, plan_guard, worktree_guard]);
    // The whip Stop hook gets the user's phrase override via a per-session file
    // it reads with `--phrases`; absent/cleared → the hook falls back to its
    // compiled defaults, so a stale file from a prior spawn is removed.
    // Exactly one `Stop` hook per session: the whip hook reports the turn end
    // itself when it lets the stop through, so a second reporter would also fire
    // on a stop whip just blocked.
    let stop_hooks = if whip {
        let arg = whip_phrases.filter(|v| !v.is_null()).map_or_else(
            || {
                remove_whip_phrases_in(root, short);
                String::new()
            },
            |v| {
                write_whip_phrases_in(root, short, v)
                    .map(|p| format!(" --phrases {}", shell_quote(&p.to_string_lossy())))
                    .unwrap_or_default()
            },
        );
        let command =
            format!("{} whip-stop-hook --sock {}{arg}", shell_quote(&exe), shell_quote(&sock));
        json!([{ "hooks": [{ "type": "command", "command": command, "timeout": 10 }] }])
    } else {
        json!([{
            "hooks": [{
                "type": "command",
                "command": ask_hook_command(&exe, "stop", &sock, false),
                "timeout": 5,
            }],
        }])
    };
    let mut hooks = json!({
        "PreToolUse": pre_hooks,
        "PostToolUse": [hook("post")],
        "Stop": stop_hooks,
    });
    let mut session_start = vec![canary_hook(&exe, &sock)];
    // Claude Code connects its MCP servers while the session starts, so a turn-1
    // `CctuiAgent` call can beat the relay's `initialize`.
    if let Some(serde_json::Value::Array(wait)) = agent_relay_session.and_then(|session| {
        let sock = crate::agenttool::socket_for_launch().to_string_lossy().into_owned();
        mcp_ready_hook(&exe, session, &sock, crate::preflight::mcp_ready_wait_secs())
    }) {
        session_start.extend(wait);
    }
    if let Some(serde_json::Value::Array(account)) =
        account_settings.and_then(|a| a.pointer("/hooks/SessionStart"))
    {
        session_start.extend(account.iter().cloned());
    }
    finish_hooks(root, short, &mut hooks, session_start, hook_policy);
    let managed = managed_settings(hooks, gateway_env, model, effort);
    // Layer the server-provided per-account settings UNDERNEATH the managed
    // settings: account keys are merged in, but the managed keys
    // (hooks, gateway env, model/effort) always win so they can never be
    // clobbered.
    let mut settings = merge_account_under_managed(managed, account_settings);
    deny_worktrees(&mut settings);
    if let Some(Err(err)) = path.parent().map(std::fs::create_dir_all) {
        tracing::warn!(%err, "ask-hook: cannot create settings dir");
        return None;
    }
    // The file carries the gateway bearer token, so it must never exist
    // world-readable, not even briefly.
    if let Err(err) =
        cctui_proto::util::write_private(&path, &serde_json::to_vec_pretty(&settings).ok()?)
    {
        tracing::warn!(%err, path = %path.display(), "ask-hook: cannot write settings");
        return None;
    }
    Some(path)
}

fn hook_policy_path(root: &std::path::Path, short: &str) -> PathBuf {
    root.join(format!("hook-policy-{short}.json"))
}

/// Install the `SessionStart` groups, then append the policy after every
/// cctui hook and keep it for a cold resume. No policy removes a stale copy
/// left by an earlier launch of `short`.
fn finish_hooks(
    root: &std::path::Path,
    short: &str,
    hooks: &mut serde_json::Value,
    session_start: Vec<serde_json::Value>,
    launch: LaunchPolicy<'_>,
) {
    hooks["SessionStart"] = serde_json::Value::Array(session_start);
    let path = hook_policy_path(root, short);
    let Some(policy) = launch.policy.filter(|p| !p.rules.is_empty()) else {
        forget_kept_policy(&path);
        let _ = std::fs::remove_file(path);
        return;
    };
    if let Err(err) = ClaudeCode.apply(policy, hooks) {
        tracing::warn!(%err, %short, "hook policy not applied");
        return;
    }
    keep_policy(root, &path, short, policy, launch.seal_key);
}

/// Policies kept by this daemon process, keyed by their on-disk path. The
/// agent can write the disk copy but not this map, so it wins while the daemon
/// that launched the session is still running.
static KEPT_POLICIES: LazyLock<KeptPolicies> = LazyLock::new(Default::default);

type KeptPolicies = std::sync::Mutex<std::collections::HashMap<PathBuf, HookPolicy>>;

fn forget_kept_policy(path: &std::path::Path) {
    if let Ok(mut kept) = KEPT_POLICIES.lock() {
        kept.remove(path);
    }
}

/// The disk copy outlives a daemon restart. It sits in the agent-writable
/// config dir, so it carries an HMAC keyed by the machine key and is only
/// written when that key is known.
fn keep_policy(
    root: &std::path::Path,
    path: &std::path::Path,
    short: &str,
    policy: &HookPolicy,
    seal_key: Option<&str>,
) {
    if let Ok(mut kept) = KEPT_POLICIES.lock() {
        kept.insert(path.to_path_buf(), policy.clone());
    }
    let (Some(key), Ok(value)) = (seal_key, serde_json::to_value(policy)) else {
        let _ = std::fs::remove_file(path);
        return;
    };
    let mac = hex::encode(policy_mac(key, short, &value));
    let doc = json!({ "policy": value, "mac": mac });
    let written = std::fs::create_dir_all(root).and_then(|()| {
        cctui_proto::util::write_private(path, &serde_json::to_vec(&doc).unwrap_or_default())
    });
    if let Err(err) = written {
        tracing::warn!(%err, path = %path.display(), "hook policy: cannot keep it for resume");
    }
}

/// The kept hook policy failed its integrity check: edited, unsealed, or
/// unreadable.
#[derive(Debug, PartialEq, Eq)]
pub(in crate::adapters::claude_code) struct TamperedPolicy;

impl std::fmt::Display for TamperedPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the kept hook policy failed its integrity check")
    }
}

impl std::error::Error for TamperedPolicy {}

/// The hook policy `short` was launched with, for a cold resume. `Ok(None)`
/// when it was launched without one.
pub(super) fn recover_hook_policy(
    short: &str,
    seal_key: Option<&str>,
) -> Result<Option<HookPolicy>, TamperedPolicy> {
    config_root().map_or(Ok(None), |root| recover_hook_policy_in(&root, short, seal_key))
}

fn recover_hook_policy_in(
    root: &std::path::Path,
    short: &str,
    seal_key: Option<&str>,
) -> Result<Option<HookPolicy>, TamperedPolicy> {
    let path = hook_policy_path(root, short);
    if let Some(policy) = KEPT_POLICIES.lock().ok().and_then(|kept| kept.get(&path).cloned()) {
        return Ok(Some(policy));
    }
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(TamperedPolicy),
    };
    let key = seal_key.ok_or(TamperedPolicy)?;
    let doc: serde_json::Value = serde_json::from_slice(&bytes).map_err(|_| TamperedPolicy)?;
    let mac = doc["mac"].as_str().and_then(|m| hex::decode(m).ok()).ok_or(TamperedPolicy)?;
    if !same_mac(&mac, &policy_mac(key, short, &doc["policy"])) {
        return Err(TamperedPolicy);
    }
    serde_json::from_value(doc["policy"].clone()).map(Some).map_err(|_| TamperedPolicy)
}

fn policy_mac(key: &str, short: &str, policy: &serde_json::Value) -> [u8; 32] {
    let body = serde_json::to_vec(policy).unwrap_or_default();
    hmac_sha256(key.as_bytes(), &[b"cctui-hook-policy\0", short.as_bytes(), b"\0", &body])
}

/// RFC 2104 HMAC over SHA-256.
fn hmac_sha256(key: &[u8], message: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest as _, Sha256};
    let mut block = [0u8; 64];
    if key.len() > block.len() {
        block[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        block[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(block.map(|b| b ^ 0x36));
    for part in message {
        inner.update(part);
    }
    let mut outer = Sha256::new();
    outer.update(block.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn same_mac(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// What a `-p` turn or a child relaunch regenerates its `--settings` file
/// from, so the agent-writable file on disk is never reused as-is.
#[derive(Debug, Clone, Default)]
pub(in crate::adapters::claude_code) struct HookPosture {
    pub whip: bool,
    pub policy: Option<HookPolicy>,
}

impl HookPosture {
    pub(in crate::adapters::claude_code) fn of(spec: &cctui_proto::adapter::SessionSpec) -> Self {
        Self {
            whip: spec.permission_mode.is_some_and(cctui_proto::adapter::PermissionMode::is_whip),
            policy: spec.hook_policy.clone(),
        }
    }

    /// The posture of a session this daemon process did not launch.
    pub(in crate::adapters::claude_code) fn recover(
        short: &str,
        seal_key: Option<&str>,
    ) -> Result<Self, TamperedPolicy> {
        let policy = recover_hook_policy(short, seal_key)?;
        Ok(Self { whip: detect_whip_from_settings(short), policy })
    }

    pub(in crate::adapters::claude_code) fn write(
        &self,
        sock: &std::path::Path,
        short: &str,
        launch: &super::LaunchEnv,
        seal_key: Option<&str>,
    ) -> Option<String> {
        self.write_in(&config_root()?, sock, short, launch, seal_key)
    }

    fn write_in(
        &self,
        root: &std::path::Path,
        sock: &std::path::Path,
        short: &str,
        launch: &super::LaunchEnv,
        seal_key: Option<&str>,
    ) -> Option<String> {
        write_hook_settings_in(
            root,
            sock,
            self.whip,
            short,
            launch.settings.as_ref(),
            &launch.env,
            None,
            None,
            launch.whip_phrases.as_ref(),
            None,
            LaunchPolicy { policy: self.policy.as_ref(), seal_key },
        )
        .map(|p| p.to_string_lossy().into_owned())
    }
}

/// Rules every managed session denies. Deny rules hold even in
/// `bypassPermissions`.
const MANAGED_DENY: [&str; 2] = ["EnterWorktree", "Bash(git worktree add:*)"];

/// Add [`MANAGED_DENY`] to `settings.permissions.deny`, keeping any rules the
/// account settings already deny: the array merge in [`deep_merge`] would
/// otherwise replace them wholesale.
fn deny_worktrees(settings: &mut serde_json::Value) {
    if !settings.is_object() {
        return;
    }
    let permissions = &mut settings["permissions"];
    if !permissions.is_object() {
        *permissions = json!({});
    }
    let deny = &mut permissions["deny"];
    if !deny.is_array() {
        *deny = json!([]);
    }
    let Some(rules) = deny.as_array_mut() else { return };
    for rule in MANAGED_DENY {
        if !rules.iter().any(|r| r.as_str() == Some(rule)) {
            rules.push(json!(rule));
        }
    }
}

/// Claude Code runs hook commands through a shell; an unquoted macOS
/// `~/Library/Application Support/...` path would split into two arguments.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn ask_hook_command(exe: &str, event: &str, sock: &str, deny: bool) -> String {
    let deny = if deny { " --deny" } else { "" };
    format!("{} ask-hook --event {event} --sock {}{deny}", shell_quote(exe), shell_quote(sock))
}

/// The `SessionStart` group that tells the daemon this worker loaded cctui's
/// hooks. No matcher, so it also runs on resume, `/clear` and compact.
fn canary_hook(exe: &str, sock: &str) -> serde_json::Value {
    json!({
        "hooks": [{
            "type": "command",
            "command": ask_hook_command(exe, "start", sock, false),
            "timeout": 5,
        }],
    })
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
                    "{} mcp-wait --session {} --sock {} --timeout {wait_secs}",
                    shell_quote(exe),
                    shell_quote(session),
                    shell_quote(sock),
                ),
                "timeout": wait_secs + 5,
            }],
        }])
    })
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
    ensure_agent_mcp_config_in(&config_root()?, short, session_id, capability)
}

fn ensure_agent_mcp_config_in(
    root: &std::path::Path,
    short: &str,
    session_id: &str,
    capability: Option<&cctui_proto::api::SpawnCapability>,
) -> Option<PathBuf> {
    if capability.is_none_or(cctui_proto::api::SpawnCapability::is_empty) {
        return None;
    }
    let path = root.join(format!("mcp-agent-{short}.json"));
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
    // The config carries the session key that authenticates to the relay
    // socket, so it must never exist world-readable, not even briefly.
    if let Err(err) =
        cctui_proto::util::write_private(&path, &serde_json::to_vec_pretty(&config).ok()?)
    {
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
    // Outranks a `disableAllHooks: true` in the user, project or local settings.
    managed.insert("disableAllHooks".to_owned(), json!(false));
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

pub(super) const BG_ISOLATION_ENV: &str = "CLAUDE_BG_ISOLATION";

/// Claude Code's background edit guard reads only the process env's
/// [`BG_ISOLATION_ENV`], never the `--settings` file, so mirror the account's
/// choice into the launch env. Precedence: the launch env, this daemon's own
/// env (a worker pod's profile), the account's `env` entry, then its
/// `worktree.bgIsolation`.
pub(super) fn with_bg_isolation(
    env: &mut std::collections::BTreeMap<String, String>,
    inherited: Option<String>,
    settings: Option<&serde_json::Value>,
) {
    if env.contains_key(BG_ISOLATION_ENV) {
        return;
    }
    let from_settings = || {
        ["/env/CLAUDE_BG_ISOLATION", "/worktree/bgIsolation"]
            .into_iter()
            .find_map(|key| {
                settings?.pointer(key)?.as_str().filter(|v| matches!(*v, "none" | "worktree"))
            })
            .map(str::to_owned)
    };
    if let Some(value) = inherited.filter(|v| !v.trim().is_empty()).or_else(from_settings) {
        env.insert(BG_ISOLATION_ENV.to_owned(), value);
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::*;

    fn isolation(
        env: &[(&str, &str)],
        inherited: Option<&str>,
        settings: Option<&serde_json::Value>,
    ) -> Option<String> {
        let mut env = env_of(env);
        with_bg_isolation(&mut env, inherited.map(str::to_owned), settings);
        env.get(BG_ISOLATION_ENV).cloned()
    }

    #[test]
    fn the_account_isolation_setting_reaches_the_launch_env() {
        let none = json!({"worktree": {"bgIsolation": "none"}});
        let worktree = json!({"worktree": {"bgIsolation": "worktree"}});
        assert_eq!(isolation(&[], None, Some(&none)).as_deref(), Some("none"));
        assert_eq!(isolation(&[], None, Some(&worktree)).as_deref(), Some("worktree"));
        let both = json!({
            "env": {"CLAUDE_BG_ISOLATION": "none"},
            "worktree": {"bgIsolation": "worktree"},
        });
        assert_eq!(
            isolation(&[], None, Some(&both)).as_deref(),
            Some("none"),
            "the env knob first"
        );
        let bad_knob = json!({
            "env": {"CLAUDE_BG_ISOLATION": "off"},
            "worktree": {"bgIsolation": "worktree"},
        });
        assert_eq!(isolation(&[], None, Some(&bad_knob)).as_deref(), Some("worktree"));
    }

    #[test]
    fn an_explicit_isolation_env_wins_over_the_account_setting() {
        let worktree = json!({"worktree": {"bgIsolation": "worktree"}});
        assert_eq!(
            isolation(&[(BG_ISOLATION_ENV, "none")], Some("worktree"), Some(&worktree)).as_deref(),
            Some("none"),
            "launch env first"
        );
        assert_eq!(
            isolation(&[], Some("none"), Some(&worktree)).as_deref(),
            Some("none"),
            "a worker profile's env beats the account setting"
        );
    }

    #[test]
    fn no_isolation_anywhere_leaves_the_env_untouched() {
        assert_eq!(isolation(&[], None, None), None);
        assert_eq!(isolation(&[], Some(" "), Some(&json!({"theme": "dark"}))), None);
        assert_eq!(isolation(&[], None, Some(&json!({"worktree": {"bgIsolation": "off"}}))), None);
    }

    #[test]
    fn only_the_matching_gateway_session_settings_authorize_a_respawn_repair() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("settings.json");
        std::fs::write(&file, json!({"env": {"CCTUI_SESSION_ID":"ours", "ANTHROPIC_AUTH_TOKEN":"cctui_s_example", "ANTHROPIC_BASE_URL":"https://gateway.test"}}).to_string()).unwrap();
        assert!(is_managed_gateway_settings(&file, "ours"));
        assert!(!is_managed_gateway_settings(&file, "another"));
        std::fs::write(&file, "{}").unwrap();
        assert!(!is_managed_gateway_settings(&file, "ours"));
    }

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

    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    fn chmod(path: &std::path::Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[test]
    fn hook_settings_land_0600_under_the_injected_root_on_create_and_rewrite() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("nested").join("cctui");
        let env = env_of(&[("ANTHROPIC_AUTH_TOKEN", "cctui_s_bearer")]);
        let write = || {
            ensure_hook_settings_in(
                &root,
                std::path::Path::new("/run/hook.sock"),
                false,
                "aaaaaaa1",
                None,
                &env,
                Some("opus"),
                Some("high"),
                None,
                None,
            )
            .expect("the injected root is writable")
        };

        let path = write();
        assert_eq!(path, root.join("hook-settings-aaaaaaa1.json"));
        assert_eq!(mode_of(&path), 0o600, "the gateway bearer token must never be readable");
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["env"]["ANTHROPIC_AUTH_TOKEN"], json!("cctui_s_bearer"));
        assert!(written["hooks"]["PreToolUse"].is_array());

        chmod(&path, 0o644);
        assert_eq!(write(), path);
        assert_eq!(mode_of(&path), 0o600, "a rewrite must restore 0600");
    }

    #[test]
    fn every_session_denies_worktrees_and_keeps_the_account_denies() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cctui");
        let account = json!({ "permissions": { "deny": ["Bash(rm:*)", "EnterWorktree"] } });
        let write = |short: &str, account: Option<&serde_json::Value>| {
            let path = ensure_hook_settings_in(
                &root,
                std::path::Path::new("/run/hook.sock"),
                false,
                short,
                account,
                &std::collections::BTreeMap::new(),
                None,
                None,
                None,
                None,
            )
            .expect("written");
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()
        };

        let plain = write("fffffff6", None);
        assert_eq!(
            plain["permissions"]["deny"],
            json!(["EnterWorktree", "Bash(git worktree add:*)"])
        );

        let merged = write("ggggggg7", Some(&account));
        assert_eq!(
            merged["permissions"]["deny"],
            json!(["Bash(rm:*)", "EnterWorktree", "Bash(git worktree add:*)"])
        );
    }

    #[test]
    fn the_agent_tool_rides_the_pre_hook_for_the_worktree_guard() {
        let tmp = tempfile::tempdir().unwrap();
        let path = ensure_hook_settings_in(
            &tmp.path().join("cctui"),
            std::path::Path::new("/run/hook.sock"),
            false,
            "hhhhhhh8",
            None,
            &std::collections::BTreeMap::new(),
            None,
            None,
            None,
            None,
        )
        .expect("written");
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let guard = written["hooks"]["PreToolUse"]
            .as_array()
            .unwrap()
            .iter()
            .find(|h| h["matcher"] == json!("Agent|Task"))
            .expect("an Agent|Task PreToolUse hook");
        let command = guard["hooks"][0]["command"].as_str().unwrap();
        assert!(command.contains("ask-hook --event pre"), "{command}");
        assert!(!command.contains("--deny"), "{command}");
    }

    #[test]
    fn every_session_installs_exactly_one_stop_hook() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cctui");
        let sock = std::path::Path::new("/run/hook.sock");
        let write = |whip: bool, short: &str| {
            let path = ensure_hook_settings_in(
                &root,
                sock,
                whip,
                short,
                None,
                &std::collections::BTreeMap::new(),
                None,
                None,
                None,
                None,
            )
            .expect("written");
            serde_json::from_slice::<serde_json::Value>(&std::fs::read(path).unwrap()).unwrap()
        };

        let plain = write(false, "ddddddd4");
        let stop = plain["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.len(), 1, "one Stop group");
        let hooks = stop[0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1, "one Stop hook, never a second reporter");
        let command = hooks[0]["command"].as_str().unwrap();
        assert!(command.contains("ask-hook --event stop"), "{command}");
        assert!(command.contains("/run/hook.sock"), "{command}");
        assert!(!command.contains("whip-stop-hook"), "{command}");
        assert!(!stop_block_is_whip(plain.pointer("/hooks/Stop")));

        let whip = write(true, "eeeeeee5");
        let hooks = whip["hooks"]["Stop"][0]["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1, "whip reports the turn end itself, so it stays alone");
        let command = hooks[0]["command"].as_str().unwrap();
        assert!(command.contains("whip-stop-hook --sock '/run/hook.sock'"), "{command}");
        assert!(!command.contains("ask-hook"), "{command}");
        assert!(
            stop_block_is_whip(whip.pointer("/hooks/Stop")),
            "the whip discriminator is the command, not the block's presence"
        );
    }

    #[test]
    fn whip_hook_settings_and_phrases_stay_inside_the_injected_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cctui");
        let phrases = json!({ "mode": "extend", "phrases": ["pour une autre session"] });
        let path = ensure_hook_settings_in(
            &root,
            std::path::Path::new("/run/hook.sock"),
            true,
            "bbbbbbb2",
            None,
            &std::collections::BTreeMap::new(),
            None,
            None,
            Some(&phrases),
            None,
        )
        .expect("written");
        assert_eq!(mode_of(&path), 0o600);
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let stop = written["hooks"]["Stop"][0]["hooks"][0]["command"].as_str().unwrap();
        assert!(stop.contains("whip-stop-hook"), "{stop}");
        let phrases_path = root.join("whip-phrases-bbbbbbb2.json");
        assert!(phrases_path.exists(), "the override file rides the injected root too");
        assert!(stop.contains(&phrases_path.to_string_lossy().into_owned()), "{stop}");
    }

    #[test]
    fn agent_mcp_config_lands_0600_under_the_injected_root() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("cctui");
        let cap = cctui_proto::api::SpawnCapability {
            adapters: vec!["claude-code".to_owned()],
            ..Default::default()
        };
        assert!(
            ensure_agent_mcp_config_in(&root, "ccccccc3", "sess-3", None).is_none(),
            "no capability writes no file"
        );
        let path = ensure_agent_mcp_config_in(&root, "ccccccc3", "sess-3", Some(&cap))
            .expect("the injected root is writable");
        assert_eq!(path, root.join("mcp-agent-ccccccc3.json"));
        assert_eq!(mode_of(&path), 0o600, "the relay session key must never be readable");
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["mcpServers"]["cctui"]["args"][2], json!("sess-3"));

        chmod(&path, 0o666);
        ensure_agent_mcp_config_in(&root, "ccccccc3", "sess-3", Some(&cap)).expect("rewritten");
        assert_eq!(mode_of(&path), 0o600, "a rewrite must restore 0600");
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

        // No env / model / effort → no `env`, `model` or `effortLevel` key.
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

    fn bare_spec() -> cctui_proto::adapter::SessionSpec {
        cctui_proto::adapter::SessionSpec {
            service_tier: None,
            adapter_id: cctui_proto::adapter::AdapterId::new("claude-code"),
            working_dir: Some("/work/cctui".to_owned()),
            prompt: None,
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            env: std::collections::BTreeMap::new(),
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
            hook_policy: None,
        }
    }

    #[test]
    fn session_context_never_mentions_other_sessions_in_the_cwd() {
        let block = build_session_context(&bare_spec(), "/work/cctui", &[], None, None);
        assert!(block.contains("cwd: /work/cctui"));
        assert!(!block.contains("shared cwd"), "{block}");
    }

    /// The staged-context notice rides the same block as everything else a
    /// claude session is told, so there is one place to read.
    #[test]
    fn the_context_notice_lands_in_the_session_block() {
        let block = build_session_context(
            &bare_spec(),
            "/work/cctui",
            &[],
            None,
            Some("context: 1 attached for this session (House style). Read `/tmp/x/context.md`.\n"),
        );
        assert!(block.contains("context: 1 attached for this session (House style)"), "{block}");
        assert!(block.contains("/tmp/x/context.md"), "{block}");
        assert!(block.ends_with("</cctui-session-context>"), "{block}");

        let bare = build_session_context(&bare_spec(), "/work/cctui", &[], None, Some("  "));
        assert!(!bare.contains("context:"), "a blank notice adds no line: {bare}");
    }

    #[test]
    fn every_session_context_says_how_to_show_an_image() {
        let block = build_session_context(&bare_spec(), "/work/cctui", &[], None, None);
        assert!(block.contains(crate::preamble::IMAGE_LINE), "{block}");
        assert!(block.ends_with("</cctui-session-context>"), "{block}");

        let cap = cctui_proto::api::SpawnCapability::machine_default();
        let block = build_session_context(&bare_spec(), "/work/cctui", &[], Some(&cap), None);
        let image = block.find("images: to show the user an image").expect("image line");
        assert!(image < block.find("CctuiAgent:").expect("agent paragraph"), "{block}");
    }

    #[test]
    fn every_session_context_says_thinking_is_not_a_reply() {
        let block = build_session_context(&bare_spec(), "/work/cctui", &[], None, None);
        assert!(block.contains(REPLY_LINE), "{block}");
        assert!(REPLY_LINE.ends_with(".\n") && REPLY_LINE.lines().count() == 1, "{REPLY_LINE}");
        let reply = block.find(REPLY_LINE).expect("reply line");
        let image = block.find(crate::preamble::IMAGE_LINE).expect("image line");
        assert_eq!(reply + REPLY_LINE.len(), image, "{block}");
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
            hook_policy: None,
        };
        let block = build_session_context(&spec, "/work/cctui", &["a.rs".to_owned()], None, None);
        assert!(block.starts_with("<cctui-session-context>\n"));
        assert!(block.ends_with("</cctui-session-context>"));
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
            hook_policy: None,
        };
        let cap = cctui_proto::api::SpawnCapability::machine_default();
        let block = build_session_context(&spec, "/work/cctui", &[], Some(&cap), None);
        assert!(block.contains("mcp__cctui__CctuiAgent"));
        assert!(block.contains("adapters you may spawn: claude-code, codex, opencode"));
        assert!(block.contains("no budget limit"), "{block}");
        assert!(block.contains("no limit on the number of children"), "{block}");
        assert!(!block.contains("per-child budget ceiling"), "{block}");
        assert!(!block.contains("max children for this session"), "{block}");
        assert!(block.contains("example: mcp__cctui__CctuiAgent({\"adapter\": \"claude-code\""));
        assert!(block.contains("mcp__cctui__CctuiUsage"), "the limits tool is announced too");
        assert!(block.contains("blocked model burns the whole batch"), "{block}");
        assert!(
            block.contains("retry the call once"),
            "turn 1 can beat the relay, so the model is told to retry: {block}"
        );
        assert!(block.ends_with("</cctui-session-context>"));

        let empty = cctui_proto::api::SpawnCapability::default();
        let block = build_session_context(&spec, "/work/cctui", &[], Some(&empty), None);
        assert!(!block.contains("CctuiAgent"), "an empty capability advertises nothing");
        assert!(!block.contains("CctuiUsage"), "the relay is absent, so neither tool exists");
    }

    fn session_start_commands(settings: &serde_json::Value) -> Vec<String> {
        settings["hooks"]["SessionStart"]
            .as_array()
            .expect("SessionStart is always registered")
            .iter()
            .flat_map(|group| group["hooks"].as_array().cloned().unwrap_or_default())
            .map(|hook| hook["command"].as_str().unwrap_or_default().to_owned())
            .collect()
    }

    fn policy_rule(
        event: cctui_proto::hook_policy::HookEvent,
        tool: cctui_proto::hook_policy::ToolClass,
        command: &str,
    ) -> cctui_proto::hook_policy::HookRule {
        cctui_proto::hook_policy::HookRule {
            event,
            tool,
            command: command.into(),
            timeout_secs: Some(9),
        }
    }

    fn write_with_policy(
        root: &std::path::Path,
        short: &str,
        account: Option<&serde_json::Value>,
        policy: Option<&HookPolicy>,
    ) -> serde_json::Value {
        let path = write_hook_settings_in(
            root,
            std::path::Path::new("/run/c.sock"),
            false,
            short,
            account,
            &std::collections::BTreeMap::new(),
            None,
            None,
            None,
            None,
            LaunchPolicy { policy, seal_key: Some(SEAL_KEY) },
        )
        .unwrap();
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }

    const SEAL_KEY: &str = "mk_test_machine_key";

    fn groups_with_matcher<'a>(
        settings: &'a serde_json::Value,
        event: &str,
        matcher: &str,
    ) -> Vec<&'a serde_json::Value> {
        settings["hooks"][event]
            .as_array()
            .map(|groups| groups.iter().filter(|g| g["matcher"] == matcher).collect())
            .unwrap_or_default()
    }

    #[test]
    fn policy_hooks_join_the_settings_with_claude_matchers_and_cctui_hooks_stay_intact() {
        use cctui_proto::hook_policy::{HookEvent, ToolClass};
        let tmp = tempfile::tempdir().unwrap();
        let policy = HookPolicy {
            rules: vec![
                policy_rule(HookEvent::PreToolUse, ToolClass::Shell, "p-shell"),
                policy_rule(HookEvent::PreToolUse, ToolClass::FileEdit, "p-edit"),
                policy_rule(HookEvent::PreToolUse, ToolClass::FileWrite, "p-write"),
                policy_rule(HookEvent::PostToolUse, ToolClass::Subagent, "p-agent"),
                policy_rule(HookEvent::PreToolUse, ToolClass::Any, "p-any"),
                policy_rule(HookEvent::SessionStart, ToolClass::Any, "p-start"),
                policy_rule(HookEvent::UserPromptSubmit, ToolClass::Any, "p-prompt"),
                policy_rule(HookEvent::Stop, ToolClass::Any, "p-stop"),
            ],
        };
        let account = json!({ "hooks": { "PreToolUse": [{ "matcher": "Bash", "hooks": [] }] } });
        let bare = write_with_policy(tmp.path(), "nopolicy", Some(&account), None);
        let merged = write_with_policy(tmp.path(), "policy01", Some(&account), Some(&policy));

        for event in ["PreToolUse", "PostToolUse", "Stop", "SessionStart"] {
            let cctui = bare["hooks"][event].as_array().unwrap();
            let all = merged["hooks"][event].as_array().unwrap();
            assert!(!cctui.is_empty(), "cctui registers {event}");
            assert_eq!(
                &all[..cctui.len()],
                &cctui[..],
                "cctui's {event} hooks come first, untouched"
            );
        }
        let command = |g: &serde_json::Value| {
            g["hooks"][0]["command"].as_str().unwrap_or_default().to_owned()
        };
        let only = |event: &str, matcher: &str| -> Vec<String> {
            groups_with_matcher(&merged, event, matcher)
                .into_iter()
                .map(command)
                .filter(|c| c.starts_with("p-"))
                .collect()
        };
        assert_eq!(only("PreToolUse", "Bash"), ["p-shell"]);
        assert_eq!(only("PreToolUse", "Edit|MultiEdit|NotebookEdit"), ["p-edit"]);
        assert_eq!(only("PreToolUse", "Write"), ["p-write"]);
        assert_eq!(only("PreToolUse", "*"), ["p-any"]);
        assert_eq!(only("PostToolUse", "Task|Agent"), ["p-agent"]);
        assert_eq!(merged["hooks"]["UserPromptSubmit"][0]["hooks"][0]["command"], "p-prompt");
        assert_eq!(merged["hooks"]["UserPromptSubmit"][0].get("matcher"), None);
        let stop = merged["hooks"]["Stop"].as_array().unwrap().last().unwrap();
        assert_eq!(
            stop,
            &json!({ "hooks": [{ "type": "command", "command": "p-stop", "timeout": 9 }] })
        );
        let starts = session_start_commands(&merged);
        assert!(starts[0].ends_with("ask-hook --event start --sock '/run/c.sock'"), "canary first");
        assert_eq!(starts.last().map(String::as_str), Some("p-start"));
        assert!(
            merged["permissions"]["deny"].as_array().unwrap().contains(&json!("EnterWorktree")),
            "managed denies survive"
        );
    }

    fn stop_policy() -> HookPolicy {
        use cctui_proto::hook_policy::{HookEvent, ToolClass};
        HookPolicy { rules: vec![policy_rule(HookEvent::Stop, ToolClass::Any, "p-stop")] }
    }

    #[test]
    fn the_policy_is_kept_for_a_cold_resume_and_cleared_without_one() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = stop_policy();
        write_with_policy(tmp.path(), "resume01", None, Some(&policy));
        assert_eq!(
            recover_hook_policy_in(tmp.path(), "resume01", Some(SEAL_KEY)),
            Ok(Some(policy))
        );
        write_with_policy(tmp.path(), "resume01", None, None);
        assert_eq!(recover_hook_policy_in(tmp.path(), "resume01", Some(SEAL_KEY)), Ok(None));
        assert!(!hook_policy_path(tmp.path(), "resume01").exists());
    }

    /// The disk copy as a restarted daemon sees it: nothing kept in memory.
    fn recover_after_restart(
        root: &std::path::Path,
        short: &str,
        key: Option<&str>,
    ) -> Result<Option<HookPolicy>, TamperedPolicy> {
        forget_kept_policy(&hook_policy_path(root, short));
        recover_hook_policy_in(root, short, key)
    }

    #[test]
    fn a_sealed_policy_survives_a_daemon_restart() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = stop_policy();
        write_with_policy(tmp.path(), "seal0001", None, Some(&policy));
        assert_eq!(recover_after_restart(tmp.path(), "seal0001", Some(SEAL_KEY)), Ok(Some(policy)));
    }

    #[test]
    fn a_tampered_on_disk_policy_is_not_trusted_on_resume() {
        let tmp = tempfile::tempdir().unwrap();
        let path = hook_policy_path(tmp.path(), "tamper01");
        let policy = stop_policy();
        write_with_policy(tmp.path(), "tamper01", None, Some(&policy));
        let mut doc: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        doc["policy"]["rules"] = json!([]);
        std::fs::write(&path, serde_json::to_vec(&doc).unwrap()).unwrap();

        assert_eq!(
            recover_hook_policy_in(tmp.path(), "tamper01", Some(SEAL_KEY)),
            Ok(Some(policy)),
            "the launching daemon's own copy wins over the edited file"
        );
        assert_eq!(
            recover_after_restart(tmp.path(), "tamper01", Some(SEAL_KEY)),
            Err(TamperedPolicy)
        );
    }

    #[test]
    fn a_policy_file_the_daemon_cannot_verify_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = hook_policy_path(tmp.path(), "forged01");
        write_with_policy(tmp.path(), "forged01", None, Some(&stop_policy()));
        assert_eq!(
            recover_after_restart(tmp.path(), "forged01", Some("other-key")),
            Err(TamperedPolicy)
        );
        assert_eq!(recover_after_restart(tmp.path(), "forged01", None), Err(TamperedPolicy));

        let unsealed = serde_json::to_vec(&stop_policy()).unwrap();
        std::fs::write(&path, unsealed).unwrap();
        assert_eq!(
            recover_after_restart(tmp.path(), "forged01", Some(SEAL_KEY)),
            Err(TamperedPolicy)
        );

        let copied = hook_policy_path(tmp.path(), "copied01");
        write_with_policy(tmp.path(), "other001", None, Some(&stop_policy()));
        std::fs::copy(hook_policy_path(tmp.path(), "other001"), &copied).unwrap();
        assert_eq!(
            recover_after_restart(tmp.path(), "copied01", Some(SEAL_KEY)),
            Err(TamperedPolicy),
            "a seal is bound to its session"
        );
    }

    #[test]
    fn without_a_seal_key_the_policy_stays_in_memory_only() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = stop_policy();
        let path = write_hook_settings_in(
            tmp.path(),
            std::path::Path::new("/run/c.sock"),
            false,
            "nokey001",
            None,
            &std::collections::BTreeMap::new(),
            None,
            None,
            None,
            None,
            LaunchPolicy { policy: Some(&policy), seal_key: None },
        );
        assert!(path.is_some());
        assert!(!hook_policy_path(tmp.path(), "nokey001").exists());
        assert_eq!(recover_hook_policy_in(tmp.path(), "nokey001", None), Ok(Some(policy)));
        assert_eq!(recover_after_restart(tmp.path(), "nokey001", None), Ok(None));
    }

    #[test]
    fn a_first_spawn_posture_carries_the_spec_policy_into_the_settings() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = std::path::Path::new("/run/c.sock");
        let launch = super::super::LaunchEnv::default();
        let spec = cctui_proto::adapter::SessionSpec {
            permission_mode: Some(cctui_proto::adapter::PermissionMode::Whip),
            hook_policy: Some(stop_policy()),
            ..launch_argv_spec()
        };
        let posture = HookPosture::of(&spec);
        assert!(posture.whip);
        assert_eq!(posture.policy, Some(stop_policy()));

        let path = posture.write_in(tmp.path(), sock, "first001", &launch, Some(SEAL_KEY)).unwrap();
        let settings: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let stop = settings["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(stop.last().unwrap()["hooks"][0]["command"], "p-stop");
        assert!(stop_block_is_whip(settings.pointer("/hooks/Stop")));
        assert_eq!(
            recover_after_restart(tmp.path(), "first001", Some(SEAL_KEY)),
            Ok(Some(stop_policy()))
        );

        let bare = HookPosture::of(&launch_argv_spec());
        assert_eq!(bare.policy, None);
        bare.write_in(tmp.path(), sock, "first002", &launch, Some(SEAL_KEY)).unwrap();
        assert!(!hook_policy_path(tmp.path(), "first002").exists());
    }

    #[test]
    fn a_turn_posture_rewrites_the_settings_and_its_policy_every_time() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = std::path::Path::new("/run/c.sock");
        let posture = HookPosture { whip: true, policy: Some(stop_policy()) };
        let launch = super::super::LaunchEnv::default();
        let path = posture.write_in(tmp.path(), sock, "turn0001", &launch, Some(SEAL_KEY)).unwrap();
        let first = std::fs::read(&path).unwrap();
        std::fs::write(&path, br#"{"disableAllHooks":true}"#).unwrap();
        std::fs::remove_file(hook_policy_path(tmp.path(), "turn0001")).unwrap();

        let again =
            posture.write_in(tmp.path(), sock, "turn0001", &launch, Some(SEAL_KEY)).unwrap();
        assert_eq!(again, path);
        assert_eq!(std::fs::read(&path).unwrap(), first);
        let settings: serde_json::Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(settings["disableAllHooks"], json!(false));
        assert!(stop_block_is_whip(settings.pointer("/hooks/Stop")));
        assert_eq!(
            recover_after_restart(tmp.path(), "turn0001", Some(SEAL_KEY)),
            Ok(Some(stop_policy()))
        );
    }

    #[test]
    fn hmac_matches_the_rfc_4231_vector() {
        let mac = hmac_sha256(b"Jefe", &[b"what do ya ", b"want for nothing?"]);
        assert_eq!(
            hex::encode(mac),
            "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843"
        );
    }

    #[test]
    fn every_generated_settings_file_pins_hooks_on() {
        let tmp = tempfile::tempdir().unwrap();
        let account = json!({ "disableAllHooks": true, "env": { "KEEP": "1" } });
        let with_account = write_with_policy(tmp.path(), "pinon001", Some(&account), None);
        assert_eq!(
            with_account["disableAllHooks"],
            json!(false),
            "account settings cannot disable"
        );
        assert_eq!(with_account["env"]["KEEP"], json!("1"));
        let bare = write_with_policy(tmp.path(), "pinon002", None, Some(&stop_policy()));
        assert_eq!(bare["disableAllHooks"], json!(false));
    }

    #[test]
    fn a_relaunch_overwrites_an_agent_edited_settings_file() {
        let tmp = tempfile::tempdir().unwrap();
        let policy = stop_policy();
        let first = write_with_policy(tmp.path(), "relnch01", None, Some(&policy));
        let path = tmp.path().join("hook-settings-relnch01.json");
        std::fs::write(&path, br#"{"disableAllHooks":true,"hooks":{}}"#).unwrap();
        let again = write_with_policy(tmp.path(), "relnch01", None, Some(&policy));
        assert_eq!(again, first);
        assert_eq!(again["disableAllHooks"], json!(false));
    }

    #[test]
    fn every_session_installs_the_session_start_canary() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = std::path::Path::new("/run/c.sock");
        let empty = std::collections::BTreeMap::new();
        for whip in [false, true] {
            let path = ensure_hook_settings_in(
                tmp.path(),
                sock,
                whip,
                "canary1",
                None,
                &empty,
                None,
                None,
                None,
                None,
            )
            .unwrap();
            let written: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
            let commands = session_start_commands(&written);
            assert_eq!(commands.len(), 1, "no relay → the canary alone: {commands:?}");
            assert!(
                commands[0].ends_with("ask-hook --event start --sock '/run/c.sock'"),
                "{}",
                commands[0]
            );
            assert!(
                written["hooks"]["SessionStart"][0].get("matcher").is_none(),
                "no matcher: the canary must run on startup, resume, clear and compact"
            );
        }
    }

    #[test]
    fn the_canary_rides_next_to_the_relay_wait() {
        let tmp = tempfile::tempdir().unwrap();
        let sock = std::path::Path::new("/run/c.sock");
        let empty = std::collections::BTreeMap::new();
        let path = ensure_hook_settings_in(
            tmp.path(),
            sock,
            false,
            "canary2",
            None,
            &empty,
            None,
            None,
            None,
            Some("sess-1"),
        )
        .unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let commands = session_start_commands(&written);
        assert!(commands[0].contains("ask-hook --event start"), "{commands:?}");
        if crate::preflight::mcp_ready_wait_secs() > 0 {
            assert_eq!(commands.len(), 2, "{commands:?}");
            assert!(commands[1].contains("mcp-wait --session 'sess-1'"), "{commands:?}");
        }
    }

    #[test]
    fn account_settings_cannot_drop_the_canary() {
        let tmp = tempfile::tempdir().unwrap();
        let account = json!({ "hooks": { "SessionStart": [] } });
        let empty = std::collections::BTreeMap::new();
        let path = ensure_hook_settings_in(
            tmp.path(),
            std::path::Path::new("/run/c.sock"),
            false,
            "canary3",
            Some(&account),
            &empty,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(session_start_commands(&written).len(), 1);
    }

    #[test]
    fn account_session_start_hooks_run_after_the_canary() {
        let tmp = tempfile::tempdir().unwrap();
        let account = json!({ "hooks": { "SessionStart": [
            { "matcher": "startup", "hooks": [{ "type": "command", "command": "acct-start" }] }
        ] } });
        let empty = std::collections::BTreeMap::new();
        let path = ensure_hook_settings_in(
            tmp.path(),
            std::path::Path::new("/run/c.sock"),
            false,
            "canary4",
            Some(&account),
            &empty,
            None,
            None,
            None,
            None,
        )
        .unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let commands = session_start_commands(&written);
        assert_eq!(commands.len(), 2, "{commands:?}");
        assert!(commands[0].contains("ask-hook --event start"), "{commands:?}");
        assert_eq!(commands[1], "acct-start");
        assert_eq!(written["hooks"]["SessionStart"][1]["matcher"], "startup");
    }

    #[test]
    fn the_session_start_hook_waits_for_the_relay_and_outlives_its_own_wait() {
        let block = mcp_ready_hook("/usr/bin/cctui-daemon", "sess-1", "/run/a.sock", 8)
            .expect("a positive wait registers the gate");
        let hook = &block[0]["hooks"][0];
        assert_eq!(
            hook["command"],
            "'/usr/bin/cctui-daemon' mcp-wait --session 'sess-1' --sock '/run/a.sock' --timeout 8"
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
    fn hook_commands_survive_the_shell_with_spaces_and_quotes_in_paths() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("Application Support");
        std::fs::create_dir(&dir).unwrap();
        let exe = dir.join("it's cctui-daemon");
        std::fs::write(&exe, "#!/bin/sh\nprintf '%s\\n' \"$@\"\n").unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let exe = exe.to_string_lossy();
        let sock = dir.join("cctui-daemon.sock").to_string_lossy().into_owned();
        let argv = |command: &str| {
            let out =
                std::process::Command::new("/bin/sh").arg("-c").arg(command).output().unwrap();
            assert!(out.status.success(), "{command}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap().lines().map(str::to_owned).collect::<Vec<_>>()
        };
        assert_eq!(
            argv(&ask_hook_command(&exe, "pre", &sock, true)),
            ["ask-hook", "--event", "pre", "--sock", sock.as_str(), "--deny"]
        );
        assert_eq!(
            argv(canary_hook(&exe, &sock)["hooks"][0]["command"].as_str().unwrap()),
            ["ask-hook", "--event", "start", "--sock", sock.as_str()]
        );
        let wait = mcp_ready_hook(&exe, "sess-1", &sock, 8).unwrap();
        assert_eq!(
            argv(wait[0]["hooks"][0]["command"].as_str().unwrap()),
            ["mcp-wait", "--session", "sess-1", "--sock", sock.as_str(), "--timeout", "8"]
        );
    }

    #[test]
    fn stage_mid_chat_files_suffixes_name_collisions() {
        use base64::Engine;
        use cctui_proto::adapter::BootstrapFile;

        let session_id = format!("test-{}", uuid::Uuid::new_v4());
        let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s.as_bytes());
        let dir = crate::adapters::uploads::session_dir(&session_id);

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
             the whole batch on 429s. Takes no arguments.\nCctuiAgentArchive: \
             `mcp__cctui__CctuiAgentArchive` archives a child this session spawned, with its \
             own children, and frees its slot: finished children keep their slot until \
             archived. A running child is killed; a session the user pinned is refused.\nThese \
             tools are served by an MCP server that connects as this session starts. If one \
             reports \"No such tool available\" on your first turn, it lost that race: wait a few seconds and retry the \
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
             no budget limit: children run uncapped unless you name a budget_usd\n  no limit on \
             the number of children\n  example: mcp__cctui__CctuiAgent({\"adapter\": \"claude-code\", \"prompt\": \"Review \
             the diff on branch X and list real defects\", \"cwd\": \
             \"/path/to/repo\"})\nCctuiUsage: `mcp__cctui__CctuiUsage` reports the rate limits \
             and budget that apply to THIS session — the account it is pinned to (possibly \
             shared or pool-elected, not necessarily your own), its usage windows, this \
             session's spend, and whether each model is currently allowed or soft-limit blocked. \
             Check it before a fan-out and when picking a child's model: a blocked model burns \
             the whole batch on 429s. Takes no arguments.\nCctuiAgentArchive: \
             `mcp__cctui__CctuiAgentArchive` archives a child this session spawned, with its \
             own children, and frees its slot: finished children keep their slot until \
             archived. A running child is killed; a session the user pinned is refused.\nThese \
             tools are served by an MCP server that connects as this session starts. If one \
             reports \"No such tool available\" on your first turn, it lost that race: wait a few seconds and retry the \
             call once before concluding the tool is missing.\n"
        );
    }
}
