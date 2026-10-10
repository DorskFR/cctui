//! Daemon side of the `CctuiAgent` and `CctuiUsage` tools.
//!
//! Listens on a local Unix socket for the `cctui-daemon mcp-agent` relay.
//! A call spawns a child through the server (the server owns the capability
//! decision — the daemon never grants anything itself), or, when it names a
//! `session_id`, sends a follow-up prompt into a child spawned earlier.
//! Both then follow the child via [`crate::childwatch`], streaming progress
//! frames to a proto≥2 relay while waiting and finishing with the child's
//! final message. Proto 1 relays (older, still attached to live sessions)
//! get the single final line only.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cctui_proto::api::{
    ArchiveChildRequest, ArchiveChildResponse, MessageChildRequest, PeerMessageRequest,
    RoomToolRequest, SpawnChildRequest,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio_util::sync::CancellationToken;

use crate::childwatch::{Assessment, WatchHandle, snippet};
use crate::client::ServerClient;

/// Cadence of progress frames to the relay while a child runs.
const PROGRESS_EVERY: Duration = Duration::from_secs(15);

/// A child that has shown no sign of life at all by this point never reached
/// its first model call — waiting out `timeout_secs` only delays the failure.
const SILENT_CHILD_GRACE: Duration = Duration::from_secs(90);

const NUDGE_PROMPT: &str =
    "continue — return your findings / final answer now, in full, and nothing else.";

/// Socket the session's MCP relay connects to. Kept beside the daemon's other
/// runtime state so a worker container with an unwritable `~/.config` still
/// finds a usable path.
#[must_use]
pub fn socket_path() -> PathBuf {
    crate::runtime::state_candidates("cctui-agent.sock")
        .into_iter()
        .next()
        .unwrap_or_else(|| std::env::temp_dir().join("cctui-agent.sock"))
}

#[derive(Debug)]
enum CallKind {
    Spawn(SpawnChildRequest),
    Message(MessageChildRequest),
    /// `CctuiUsage`: ask the server what limits apply to the calling session.
    /// Neither spawns nor follows anything, so it never touches the watch.
    Usage {
        model: Option<String>,
    },
    /// `CctuiAgentArchive`: archive a descendant and free its slot.
    ArchiveChild(ArchiveChildRequest),
    /// `CctuiPeers`: the sessions this one may address.
    Peers,
    /// `CctuiSend`: one message into a peer's turn queue. Unlike
    /// [`Self::Message`] it never follows the target — a peer is not a child and
    /// owes the caller no answer.
    SendPeer(PeerMessageRequest),
    /// `CctuiRoom`: post to, read or inspect a room this session is in.
    Room(RoomToolRequest),
    /// `CctuiHistory`: a bounded page of a peer's transcript. The peer is the
    /// `session_id` entry of `query`.
    PeerHistory {
        query: Vec<(&'static str, String)>,
    },
    /// `CctuiUserActionAdd` / `CctuiUserActionTick`: the body is forwarded to the
    /// server, which owns the list and answers with all of it.
    UserActionAdd(Value),
    UserActionTick(Value),
    /// `CctuiSpeak`: synthesize `text` into a voice note posted in the caller's
    /// own conversation.
    Speak {
        text: String,
        voice: Option<String>,
    },
    /// The relay announcing that it answered `initialize`.
    RelayReady,
    /// The session's `SessionStart` hook holding the first turn until the relay
    /// is ready.
    RelayWait,
    PreviewOpen {
        port: u16,
    },
    PreviewClose {
        port: u16,
    },
}

#[derive(Debug)]
struct Call {
    session_id: String,
    kind: CallKind,
    timeout: Duration,
    /// Relay protocol: ≥2 understands interim `progress` frames.
    proto: u64,
}

/// Where a caller finds a current model id. Concrete ids are never compiled in:
/// they go stale and steer every caller onto a superseded model.
const MODEL_ID_HINT: &str = "use your own model id (your environment names it) or one \
    CctuiUsage lists under per_model; an alias from the account's own catalog is also \
    accepted";

fn missing_model_error() -> String {
    format!(
        "model is required and was not given. CctuiAgent never falls back to the account \
         default: that silently spends a different budget than the caller intended, and a \
         whole fan-out can die on 429 minutes later without the cause being visible. Pass \
         model explicitly — {MODEL_ID_HINT}."
    )
}

fn parse_call(line: &str) -> Result<Call, String> {
    let v: Value = serde_json::from_str(line).map_err(|e| format!("malformed request: {e}"))?;
    let session_id = v
        .get("session_id")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .ok_or("request carries no session id")?
        .to_owned();
    let session_id = resolve_session_alias(&session_id);
    let args = v.get("args").cloned().unwrap_or_else(|| json!({}));
    let timeout = crate::mcp::resolve_timeout(v.get("timeout_secs").and_then(Value::as_u64));
    let proto = v.get("proto").and_then(Value::as_u64).unwrap_or(1);
    let kind = match v.get("kind").and_then(Value::as_str) {
        Some("usage") => CallKind::Usage { model: string_arg(&args, "model") },
        Some("archive_child") => match string_arg(&args, "session_id") {
            Some(session_id) => CallKind::ArchiveChild(ArchiveChildRequest { session_id }),
            None => return Err("session_id is required: the child to archive".to_owned()),
        },
        Some("peers") => CallKind::Peers,
        Some("speak") => parse_speak(&args)?,
        Some("send_peer") => parse_send_peer(&args)?,
        Some("room") => parse_room(&args)?,
        Some("peer_history") => parse_peer_history(&args)?,
        Some("user_action_add") => parse_user_action_add(&args),
        Some("user_action_tick") => parse_user_action_tick(&args)?,
        Some("relay_ready") => CallKind::RelayReady,
        Some("relay_wait") => CallKind::RelayWait,
        Some(kind @ ("preview_open" | "preview_close")) => {
            let port = args
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|p| u16::try_from(p).ok())
                .ok_or("port is required")?;
            crate::preview::validate_port(port)?;
            if kind == "preview_open" {
                CallKind::PreviewOpen { port }
            } else {
                CallKind::PreviewClose { port }
            }
        }
        Some("spawn_agent") => parse_spawn_agent(&args)?,
        _ => return Err("unsupported request kind".to_owned()),
    };
    Ok(Call { session_id, kind, timeout, proto })
}

fn parse_send_peer(args: &Value) -> Result<CallKind, String> {
    let Some(target) = string_arg(args, "session_id") else {
        return Err("session_id is required: the peer to send to".to_owned());
    };
    let message = args.get("message").and_then(Value::as_str).unwrap_or("").trim().to_owned();
    if message.is_empty() {
        return Err("message is required".to_owned());
    }
    Ok(CallKind::SendPeer(PeerMessageRequest { session_id: target, message }))
}

fn parse_room(args: &Value) -> Result<CallKind, String> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .map(|a| a.trim().to_ascii_lowercase())
        .filter(|a| !a.is_empty())
        .ok_or("action is required: \"post\", \"peek\" or \"members\"")?;
    let message = string_arg(args, "message");
    if action == "post" && message.is_none() {
        return Err("message is required to post to a room".to_owned());
    }
    Ok(CallKind::Room(RoomToolRequest { action, room_id: string_arg(args, "room_id"), message }))
}

fn parse_peer_history(args: &Value) -> Result<CallKind, String> {
    let Some(target) = string_arg(args, "session_id") else {
        return Err("session_id is required: the peer whose history to read".to_owned());
    };
    let mut query: Vec<(&'static str, String)> = vec![("session_id", target)];
    for key in ["after", "before", "limit"] {
        if let Some(n) = args.get(key).and_then(Value::as_i64) {
            query.push((key, n.to_string()));
        }
    }
    if let Some(roles) = roles_arg(args) {
        query.push(("roles", roles));
    }
    if let Some(format) = string_arg(args, "format") {
        query.push(("format", format.to_ascii_lowercase()));
    }
    Ok(CallKind::PeerHistory { query })
}

fn parse_user_action_add(args: &Value) -> CallKind {
    CallKind::UserActionAdd(json!({
        "title": args.get("title").and_then(Value::as_str).unwrap_or("").trim(),
        "detail": string_arg(args, "detail"),
        "kind": match string_arg(args, "kind").as_deref() {
            Some("input") => "input",
            Some("decision") => "decision",
            _ => "action",
        },
        "blocking": args.get("blocking").and_then(Value::as_bool).unwrap_or(false),
    }))
}

fn parse_user_action_tick(args: &Value) -> Result<CallKind, String> {
    let id = string_arg(args, "id").ok_or("id is required")?;
    let status = match string_arg(args, "status").as_deref() {
        Some("done") => "done",
        Some("dropped") => "dropped",
        other => {
            return Err(format!(
                "status must be \"done\" or \"dropped\", got {}",
                other.unwrap_or("nothing"),
            ));
        }
    };
    Ok(CallKind::UserActionTick(json!({
        "id": id,
        "status": status,
        "note": string_arg(args, "note"),
    })))
}

fn parse_spawn_agent(args: &Value) -> Result<CallKind, String> {
    let prompt = args.get("prompt").and_then(Value::as_str).unwrap_or("").to_owned();
    if prompt.trim().is_empty() {
        return Err("prompt is required".to_owned());
    }
    if let Some(child) = string_arg(args, "session_id") {
        return Ok(CallKind::Message(MessageChildRequest { session_id: child, prompt }));
    }
    let adapter = normalize_adapter(args.get("adapter").and_then(Value::as_str).unwrap_or(""));
    let Some(model) = string_arg(args, "model") else {
        return Err(missing_model_error());
    };
    Ok(CallKind::Spawn(SpawnChildRequest {
        adapter,
        prompt,
        model: Some(model),
        agent_profile: string_arg(args, "agent_profile"),
        budget_usd: args.get("budget_usd").and_then(Value::as_f64),
        cwd: string_arg(args, "cwd"),
        permission_mode: string_arg(args, "permission_mode")
            .and_then(|m| serde_json::from_value(Value::String(m)).ok()),
        name: string_arg(args, "name"),
    }))
}

/// `roles` as the comma list the route expects, accepting either the array a
/// model usually sends or a pre-joined string.
fn roles_arg(args: &Value) -> Option<String> {
    let raw = args.get("roles")?;
    let joined = match raw {
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::trim)
            .filter(|r| !r.is_empty())
            .collect::<Vec<_>>()
            .join(","),
        Value::String(s) => s.trim().to_owned(),
        _ => return None,
    };
    (!joined.is_empty()).then_some(joined)
}

fn parse_speak(args: &Value) -> Result<CallKind, String> {
    let text = string_arg(args, "text").ok_or("text is required: what to say")?;
    let max = crate::mcp::SPEAK_MAX_CHARS;
    if text.chars().count() > max {
        return Err(format!("text is too long: at most {max} characters"));
    }
    Ok(CallKind::Speak { text, voice: string_arg(args, "voice") })
}

fn string_arg(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Accept the model-facing spellings of an adapter id and return the canonical
/// one. `claude_code`/`claude` are the ids a model is most likely to guess.
#[must_use]
pub fn normalize_adapter(raw: &str) -> String {
    match raw.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "claude" | "claude-code" => "claude-code".to_owned(),
        "codex" | "codex-cli" => "codex".to_owned(),
        "gemini" | "gemini-cli" => "gemini".to_owned(),
        other => other.to_owned(),
    }
}

fn reply_frame(outcome: &crate::childwatch::ChildOutcome) -> Value {
    let id_line = outcome
        .local_id
        .as_deref()
        .map(|id| format!("\n\n[child session id: {id} — pass it as session_id to follow up]"))
        .unwrap_or_default();
    match (&outcome.error, &outcome.final_text) {
        (Some(err), Some(text)) => json!({
            "ok": false,
            "error": format!("child agent failed: {err}\n\nlast output:\n{text}{id_line}"),
        }),
        (Some(err), None) => {
            json!({ "ok": false, "error": format!("child agent failed: {err}{id_line}") })
        }
        (None, Some(text)) => json!({ "ok": true, "result": format!("{text}{id_line}") }),
        (None, None) => json!({
            "ok": true,
            "result": format!("child agent finished without producing any output{id_line}"),
        }),
    }
}

/// Appended to every frame a call returns: the caller must be able to see the
/// model it got rather than the one it assumed.
fn dispatch_note(kind: &CallKind, timeout: Duration) -> String {
    match kind {
        CallKind::Spawn(req) => format!(
            "\n\n[spawned on model {} · adapter {} · follow window {}s]",
            req.model.as_deref().unwrap_or("<unset>"),
            req.adapter,
            timeout.as_secs(),
        ),
        CallKind::Message(req) => format!(
            "\n\n[follow-up to child {} · runs on the child's original model, `model` is ignored here · follow window {}s]",
            req.session_id,
            timeout.as_secs(),
        ),
        CallKind::Usage { .. }
        | CallKind::ArchiveChild(_)
        | CallKind::Peers
        | CallKind::SendPeer(_)
        | CallKind::Room(_)
        | CallKind::PeerHistory { .. }
        | CallKind::UserActionAdd(_)
        | CallKind::UserActionTick(_)
        | CallKind::Speak { .. }
        | CallKind::RelayReady
        | CallKind::RelayWait
        | CallKind::PreviewOpen { .. }
        | CallKind::PreviewClose { .. } => String::new(),
    }
}

/// The roster as the compact lines a model reads before choosing a peer.
fn render_peers(v: &Value) -> String {
    let peers = v.get("peers").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    if peers.is_empty() {
        return "no addressable peers: this session has no parent, no children, no siblings and \
                no shared sessions"
            .to_owned();
    }
    let mut lines = vec![format!("{} addressable peer(s):", peers.len())];
    for p in peers {
        let s = |k: &str| p.get(k).and_then(Value::as_str).unwrap_or("?");
        let name = p.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty());
        if s("relation").starts_with("remote") {
            lines.push(format!(
                "- {} [{}] {} · on another cctui ({}) · {}",
                s("session_id"),
                s("relation"),
                name.unwrap_or("(unnamed)"),
                s("machine"),
                s("state"),
            ));
            continue;
        }
        lines.push(format!(
            "- {} [{}] {} · {} on {} · {}",
            s("session_id"),
            s("relation"),
            name.unwrap_or("(unnamed)"),
            s("adapter"),
            s("machine"),
            s("state"),
        ));
    }
    lines.join("\n")
}

/// A room reply, rendered per action. `post` confirms the reach so a model knows
/// how many agents it just interrupted; `peek` and `members` render the timeline
/// and the roster as lines.
fn render_sent(to: &str, relation: &str, status: &str) -> String {
    let head = match status {
        "queued" => format!(
            "queued for {to} ({relation}): the other cctui did not take it yet and it is retried \
             in the background."
        ),
        "awaiting_review" => format!(
            "held for {to} ({relation}): your owner reviews outbound messages on this link \
             before they leave."
        ),
        _ => format!("delivered to {to} ({relation})."),
    };
    format!(
        "{head} It arrives as a turn in that session; it will not reply through this tool — \
         watch for its own CctuiSend back."
    )
}

fn render_room(action: &str, me: &str, v: &Value) -> String {
    let room = v.get("room").and_then(Value::as_str).unwrap_or("the room");
    match action {
        "post" if v.get("remote").and_then(Value::as_bool) == Some(true) => {
            let status = v.get("status").and_then(Value::as_str).unwrap_or("?");
            format!(
                "posted to {room}, a room hosted on another cctui ({status}); its host delivers \
                 it to the members. They answer when they choose — nothing comes back through \
                 this call."
            )
        }
        "post" => {
            let seq = v.get("seq").and_then(Value::as_i64).unwrap_or(0);
            let delivered = v.get("delivered").and_then(Value::as_u64).unwrap_or(0);
            let receipts =
                v.get("receipts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let mut out = format!(
                "posted to {room} as #{seq}; delivered to {delivered} of {} other session(s). \
                 They answer when they choose — nothing comes back through this call.",
                receipts.len(),
            );
            // Name who did NOT get it: a broadcast is best effort, and a silent
            // skip would read as a delivery.
            let missed: Vec<String> = receipts
                .iter()
                .filter(|r| r.get("outcome").and_then(Value::as_str) != Some("delivered"))
                .map(|r| {
                    format!(
                        "{} ({})",
                        r.get("label").and_then(Value::as_str).unwrap_or("?"),
                        r.get("outcome").and_then(Value::as_str).unwrap_or("?"),
                    )
                })
                .collect();
            if !missed.is_empty() {
                let _ = write!(out, "\nNot delivered: {}.", missed.join(", "));
            }
            out
        }
        "members" => {
            let members =
                v.get("members").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            let mut lines = vec![format!("{room} — {} member(s), plus the human:", members.len())];
            for mem in members {
                if let Some(label) = mem.as_str() {
                    lines.push(format!("- {label}"));
                    continue;
                }
                let s = |k: &str| mem.get(k).and_then(Value::as_str).unwrap_or("?");
                if mem.get("remote").and_then(Value::as_bool) == Some(true) {
                    let name = mem.get("name").and_then(Value::as_str).unwrap_or("(unnamed)");
                    lines.push(format!(
                        "- {} [remote] {name} · on another cctui ({}) · {}",
                        s("session_id"),
                        s("machine"),
                        s("state"),
                    ));
                    continue;
                }
                let name = mem.get("name").and_then(Value::as_str).filter(|n| !n.trim().is_empty());
                lines.push(format!(
                    "- {} [{}] {} · {} on {} · {}",
                    s("session_id"),
                    s("role"),
                    name.unwrap_or("(unnamed)"),
                    s("adapter"),
                    s("machine"),
                    s("state"),
                ));
            }
            lines.join("\n")
        }
        _ => {
            let messages =
                v.get("messages").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
            if messages.is_empty() {
                return format!("{room} has no messages yet.");
            }
            let mut lines = vec![format!("{room} — {} message(s):", messages.len())];
            for msg in messages {
                let seq = msg.get("seq").and_then(Value::as_i64).unwrap_or(0);
                let from = msg.get("sender_label").and_then(Value::as_str).unwrap_or("?");
                let body = msg.get("body").and_then(Value::as_str).unwrap_or("");
                let mine =
                    msg.get("sender_session_id").and_then(Value::as_str).is_some_and(|s| s == me);
                let mark = if mine { " (you)" } else { "" };
                lines.push(format!("#{seq} {from}{mark}: {}", snippet(body, 2_000)));
            }
            lines.join("\n")
        }
    }
}

/// A history page as the markdown the tool promised, with the cursor line a
/// caller needs to ask for the previous page.
fn render_history(v: &Value) -> String {
    let n = v.get("events").and_then(Value::as_u64).unwrap_or(0);
    let body = v.get("markdown").and_then(Value::as_str).map_or_else(
        || serde_json::to_string_pretty(v.get("items").unwrap_or(&Value::Null)).unwrap_or_default(),
        str::to_owned,
    );
    let mut note = format!("\n\n[{n} event(s)");
    if let Some(first) = v.get("first_seq").and_then(Value::as_i64) {
        let _ = write!(note, " · oldest seq {first}");
        if v.get("truncated").and_then(Value::as_bool).unwrap_or(false) {
            let _ = write!(
                note,
                " · truncated: call again with before={first} for the page before this one"
            );
        }
    }
    note.push(']');
    format!("{body}{note}")
}

fn annotate(mut frame: Value, note: &str) -> Value {
    let key =
        if frame.get("ok").and_then(Value::as_bool) == Some(true) { "result" } else { "error" };
    if let Some(text) = frame.get(key).and_then(Value::as_str) {
        let joined = format!("{text}{note}");
        frame[key] = Value::String(joined);
    }
    frame
}

enum FollowResult {
    Finished(crate::childwatch::ChildOutcome),
    Error(Value),
}

fn follow_result_to_frame(result: FollowResult) -> Value {
    match result {
        FollowResult::Finished(outcome) => reply_frame(&outcome),
        FollowResult::Error(frame) => frame,
    }
}

/// Whether a completed turn's final message reads as a truncated non-answer
/// rather than a deliverable: empty, or ending on a planning/intent tail (a
/// trailing colon, or a bare enumeration marker the model never filled in).
fn looks_truncated(final_text: Option<&str>) -> bool {
    let Some(text) = final_text.map(str::trim).filter(|t| !t.is_empty()) else {
        return true;
    };
    let last = text.lines().rev().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    last.ends_with(':') || is_bare_enumeration_marker(last)
}

fn is_bare_enumeration_marker(line: &str) -> bool {
    matches!(line, "-" | "*" | "•") || {
        let rest = line.trim_start_matches(|c: char| c.is_ascii_digit());
        rest.len() < line.len() && matches!(rest, "." | ")")
    }
}

/// Whether a finished child warrants the single automatic continuation nudge:
/// only a clean turn (no error) qualifies — a crashed or errored child is
/// never nudged. A turn whose tail was a thinking block is nudged even when
/// the held final text reads complete: that text is stale mid-turn narration.
fn should_nudge(outcome: &crate::childwatch::ChildOutcome) -> bool {
    outcome.error.is_none()
        && (outcome.tail_is_thinking || looks_truncated(outcome.final_text.as_deref()))
}

/// Whether the child has produced any evidence of a running turn: a bound
/// session id alone only proves the harness registered it.
const fn showed_activity(snap: &crate::childwatch::ChildSnapshot) -> bool {
    snap.final_text.is_some()
        || snap.last_tool.is_some()
        || snap.status_line.is_some()
        || snap.blocked.is_some()
}

async fn follow_child_with(
    handle: &WatchHandle,
    child_id: &str,
    timeout: Duration,
    silent_grace: Duration,
    proto: u64,
    out: &mut (impl AsyncWriteExt + Unpin),
) -> FollowResult {
    let started = Instant::now();
    let mut last_progress = Instant::now();
    loop {
        handle.changed(Duration::from_secs(2)).await;
        let now = Instant::now();
        let Some(snap) = handle.snapshot() else {
            return FollowResult::Error(
                json!({ "ok": false, "error": "child agent tracking was dropped" }),
            );
        };
        match snap.assess(now) {
            Assessment::Finished(outcome) => return FollowResult::Finished(outcome),
            Assessment::Running(line) => {
                if now.duration_since(started) >= silent_grace && !showed_activity(&snap) {
                    return FollowResult::Error(json!({
                        "ok": false,
                        "error": format!(
                            "child agent {} produced no activity within {}s of being prompted — \
                             no model call, no output and no error, so it almost certainly died \
                             at startup (auth, budget or rate-limit rejection). Not waiting out \
                             the {}s timeout; check the child session in cctui.",
                            snap.local_id.as_deref().unwrap_or(child_id),
                            silent_grace.as_secs(),
                            timeout.as_secs(),
                        ),
                    }));
                }
                if now.duration_since(started) >= timeout {
                    return FollowResult::Error(json!({
                        "ok": false,
                        "error": format!(
                            "the {}s follow window expired for child agent {child_id}. THIS IS \
                             NOT A CRASH: the child is still running, and whatever it has \
                             already written is on disk. Only the wait gave up. Watch it in \
                             cctui, or call CctuiAgent again with session_id {:?} to reattach \
                             and collect its answer. Pass a larger timeout_secs (max 7200) \
                             next time to wait longer.",
                            timeout.as_secs(),
                            snap.local_id.as_deref().unwrap_or(child_id),
                        ),
                    }));
                }
                if proto >= 2 && now.duration_since(last_progress) >= PROGRESS_EVERY {
                    last_progress = now;
                    let frame = json!({
                        "progress": format!(
                            "[{}s] {} · child session {}",
                            now.duration_since(started).as_secs(),
                            snippet(&line, 300),
                            snap.local_id.as_deref().unwrap_or(child_id),
                        ),
                    });
                    if write_line(out, &frame).await.is_err() {
                        return FollowResult::Error(
                            json!({ "ok": false, "error": "relay went away" }),
                        );
                    }
                }
            }
        }
    }
}

async fn write_line(out: &mut (impl AsyncWriteExt + Unpin), frame: &Value) -> std::io::Result<()> {
    out.write_all(format!("{frame}\n").as_bytes()).await?;
    out.flush().await
}

/// `h`m / `m`m / `s`s, in the compact spelling the one-line rendering uses.
fn human_duration(secs: i64) -> String {
    match secs {
        s if s <= 0 => "now".to_owned(),
        s if s >= 3600 => format!("{}h{:02}", s / 3600, (s % 3600) / 60),
        s if s >= 60 => format!("{}m", s / 60),
        s => format!("{s}s"),
    }
}

fn secs_until(iso: &str) -> Option<i64> {
    let at = chrono::DateTime::parse_from_rfc3339(iso).ok()?;
    Some((at.with_timezone(&chrono::Utc) - chrono::Utc::now()).num_seconds())
}

#[allow(clippy::cast_possible_truncation)]
fn round_pct(v: f64) -> String {
    format!("{}%", v.round() as i64)
}

/// A dollar cap as a human writes it: `$20`, not `$20.00`, but `$7.50` intact.
fn money(v: f64) -> String {
    if (v - v.round()).abs() < 0.005 { format!("{:.0}", v.round()) } else { format!("{v:.2}") }
}

/// Condense a limits payload into the single line a model reads first.
fn render_usage(v: &Value) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(name) = v.pointer("/account/name").and_then(Value::as_str) {
        let emoji = v.pointer("/account/emoji").and_then(Value::as_str).unwrap_or("");
        parts.push(format!("{emoji}{name}").trim().to_owned());
    }
    for w in v.get("windows").and_then(Value::as_array).into_iter().flatten() {
        let label = w
            .get("label")
            .and_then(Value::as_str)
            .or_else(|| w.get("key").and_then(Value::as_str))
            .unwrap_or("window");
        let Some(util) = w.get("utilization").and_then(Value::as_f64) else { continue };
        let resets = w
            .get("resets_at")
            .and_then(Value::as_str)
            .and_then(secs_until)
            .map(|s| format!(" resets in {}", human_duration(s)))
            .unwrap_or_default();
        parts.push(format!("{label} {}{resets}", round_pct(util)));
    }
    if let Some(cap) = v.pointer("/caps/session_usd/cap_usd").and_then(Value::as_f64) {
        let spent = v.pointer("/spend/session_usd").and_then(Value::as_f64).unwrap_or(0.0);
        parts.push(format!("budget ${spent:.2}/${}", money(cap)));
    }
    if let Some(models) = v.get("per_model").and_then(Value::as_object) {
        let mut names: Vec<&String> = models.keys().collect();
        names.sort();
        for name in names {
            let d = &models[name];
            if d.get("allow").and_then(Value::as_bool).unwrap_or(true) {
                parts.push(format!("{name} ok"));
            } else {
                let key = d.get("key").and_then(Value::as_str).unwrap_or("limit");
                let retry = d
                    .get("retry_after_secs")
                    .and_then(Value::as_i64)
                    .map(|s| format!(" for {}", human_duration(s)))
                    .unwrap_or_default();
                parts.push(format!("{name} {key} BLOCKED{retry}"));
            }
        }
    }
    if let Some(used) = v.pointer("/children/used").and_then(Value::as_u64) {
        match v.pointer("/children/max").and_then(Value::as_u64) {
            Some(max) => parts.push(format!("children {used}/{max}")),
            None => parts.push(format!("children {used}")),
        }
    }
    if v.get("stale").and_then(Value::as_bool).unwrap_or(false) {
        parts.push("usage cache stale — numbers may be out of date".to_owned());
    }
    if parts.is_empty() {
        return "no usage information is available for this session".to_owned();
    }
    parts.join(" · ")
}

fn render_archived(resp: &ArchiveChildResponse) -> String {
    let slots = resp.max_children.map_or_else(
        || format!("{} child slots used", resp.children_used),
        |max| format!("{}/{max} child slots used", resp.children_used),
    );
    if resp.archived.is_empty() {
        return format!("nothing archived · {slots}");
    }
    format!("archived {} · {slots}", resp.archived.join(", "))
}

async fn run_usage(
    server: &ServerClient,
    machine_key: &str,
    session_id: &str,
    model: Option<&str>,
) -> Value {
    match server.session_limits(machine_key, session_id, model).await {
        Ok(limits) => {
            let line = render_usage(&limits);
            let detail =
                serde_json::to_string_pretty(&limits).unwrap_or_else(|_| limits.to_string());
            json!({ "ok": true, "result": format!("{line}\n\n{detail}") })
        }
        Err(err) => json!({ "ok": false, "error": err.to_string() }),
    }
}

/// One list line as the model reads it back. The resolver is always named: a
/// `done` the user ticked in the UI is the whole point of returning the list.
fn render_user_action(item: &Value) -> String {
    let title = item.get("title").and_then(Value::as_str).unwrap_or("(untitled)");
    let status = item.get("status").and_then(Value::as_str).unwrap_or("open");
    let kind = item.get("kind").and_then(Value::as_str).unwrap_or("action");
    let id = item.get("id").and_then(Value::as_str).unwrap_or("");
    let mut line = if status == "open" {
        let blocking = if item.get("blocking").and_then(Value::as_bool).unwrap_or(false) {
            " BLOCKING"
        } else {
            ""
        };
        format!("[ ]{blocking} {title} ({kind}, id {id})")
    } else {
        let by = item
            .get("resolved_by")
            .and_then(Value::as_str)
            .map(|b| format!(" by {b}"))
            .unwrap_or_default();
        format!("[{status}{by}] {title} (id {id})")
    };
    if let Some(note) = item.get("note").and_then(Value::as_str).filter(|n| !n.is_empty()) {
        let _ = write!(line, " — {note}");
    }
    line
}

/// Render a `UserActionResult` for the model: the rejection reason if any, then
/// the whole list, so a caller never has to guess what the user changed.
fn render_user_action_result(v: &Value) -> String {
    let mut out = Vec::new();
    if let Some(err) = v.get("error").and_then(Value::as_str) {
        out.push(format!("rejected: {err}"));
    }
    if let Some(added) = v.get("added").and_then(Value::as_str) {
        out.push(format!("item id: {added}"));
    }
    let items: Vec<&Value> = v
        .pointer("/list/items")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default();
    if items.is_empty() {
        out.push("the user action list is empty".to_owned());
    } else {
        let open = items
            .iter()
            .filter(|i| i.get("status").and_then(Value::as_str) == Some("open"))
            .count();
        out.push(format!("user actions ({open} open of {}):", items.len()));
        out.extend(items.iter().map(|i| render_user_action(i)));
    }
    out.join("\n")
}

async fn run_user_action(
    server: &ServerClient,
    machine_key: &str,
    session_id: &str,
    path: &str,
    body: &Value,
) -> Value {
    match server.user_action_call(machine_key, session_id, path, body).await {
        Ok(result) => {
            let raw = serde_json::to_value(&result).unwrap_or_else(|_| json!({}));
            let text = render_user_action_result(&raw);
            if result.error.is_some() {
                json!({ "ok": false, "error": text })
            } else {
                json!({ "ok": true, "result": text })
            }
        }
        Err(err) => json!({ "ok": false, "error": err.to_string() }),
    }
}

/// The kinds that answer from one server round-trip and never follow a child.
/// `None` means the call is a spawn or a follow-up, which [`run_call`] handles.
async fn run_unfollowed_call(
    server: &ServerClient,
    machine_key: &str,
    call: &Call,
) -> Option<Value> {
    let me = call.session_id.as_str();
    let frame = match &call.kind {
        CallKind::Usage { model } => run_usage(server, machine_key, me, model.as_deref()).await,
        CallKind::ArchiveChild(req) => match server.archive_child(machine_key, me, req).await {
            Ok(resp) => json!({ "ok": true, "result": render_archived(&resp) }),
            Err(err) => json!({ "ok": false, "error": err.to_string() }),
        },
        CallKind::Peers => match server.peers(machine_key, me).await {
            Ok(v) => json!({ "ok": true, "result": render_peers(&v) }),
            Err(err) => json!({ "ok": false, "error": err.to_string() }),
        },
        CallKind::SendPeer(req) => match server.message_peer(machine_key, me, req).await {
            Ok(v) => {
                let to = v.get("delivered_to").and_then(Value::as_str).unwrap_or(&req.session_id);
                let relation = v.get("relation").and_then(Value::as_str).unwrap_or("peer");
                let status = v.get("status").and_then(Value::as_str).unwrap_or("delivered");
                json!({ "ok": true, "result": render_sent(to, relation, status) })
            }
            Err(err) => json!({ "ok": false, "error": err.to_string() }),
        },
        CallKind::Room(req) => match server.room(machine_key, me, req).await {
            Ok(v) => json!({ "ok": true, "result": render_room(&req.action, me, &v) }),
            Err(err) => json!({ "ok": false, "error": err.to_string() }),
        },
        CallKind::PeerHistory { query } => {
            match server.peer_conversation(machine_key, me, query).await {
                Ok(v) => json!({ "ok": true, "result": render_history(&v) }),
                Err(err) => json!({ "ok": false, "error": err.to_string() }),
            }
        }
        CallKind::UserActionAdd(body) => run_user_action(server, machine_key, me, "", body).await,
        CallKind::UserActionTick(body) => {
            run_user_action(server, machine_key, me, "/tick", body).await
        }
        CallKind::Speak { text, voice } => {
            match server.speak(machine_key, me, text, voice.as_deref()).await {
                Ok(v) => {
                    let duration = v.get("duration_s").cloned().unwrap_or(Value::Null);
                    json!({ "ok": true, "result": json!({ "ok": true, "duration_s": duration }).to_string() })
                }
                Err(err) => json!({ "ok": false, "error": err.to_string() }),
            }
        }
        CallKind::RelayReady => {
            crate::mcpready::announce(me);
            json!({ "ok": true, "result": "ready" })
        }
        // Never `ok: false`: the hook releases the first turn either way.
        CallKind::RelayWait => {
            let ready = crate::mcpready::wait_until_ready(me, call.timeout).await;
            let result = if ready { "ready" } else { "timeout" };
            json!({ "ok": true, "result": result })
        }
        CallKind::PreviewOpen { port } => match crate::preview::open(me, *port).await {
            Ok(opened) => json!({ "ok": true, "result": opened.url }),
            Err(err) => json!({ "ok": false, "error": err }),
        },
        CallKind::PreviewClose { port } => match crate::preview::close(me, *port).await {
            Ok(()) => json!({ "ok": true, "result": "closed" }),
            Err(err) => json!({ "ok": false, "error": err }),
        },
        CallKind::Spawn(_) | CallKind::Message(_) => return None,
    };
    Some(frame)
}

async fn run_call(
    server: &ServerClient,
    machine_key: &str,
    call: Call,
    out: &mut (impl AsyncWriteExt + Unpin),
) -> Value {
    if let Some(frame) = run_unfollowed_call(server, machine_key, &call).await {
        return frame;
    }
    let note = dispatch_note(&call.kind, call.timeout);
    let watch = crate::childwatch::global();
    let (handle, child_id) = match &call.kind {
        CallKind::Spawn(req) => {
            let child = match server.spawn_child(machine_key, &call.session_id, req).await {
                Ok(child) => child,
                Err(err) => {
                    return annotate(json!({ "ok": false, "error": err.to_string() }), &note);
                }
            };
            let handle = watch.register(&child.session_id);
            tracing::info!(
                parent = %call.session_id,
                child = %child.session_id,
                adapter = %req.adapter,
                "CctuiAgent following spawned child",
            );
            (handle, child.session_id)
        }
        CallKind::Message(req) => {
            let handle = watch.register_bound(&req.session_id);
            if let Err(err) = server.message_child(machine_key, &call.session_id, req).await {
                return annotate(json!({ "ok": false, "error": err.to_string() }), &note);
            }
            tracing::info!(
                parent = %call.session_id,
                child = %req.session_id,
                "CctuiAgent following child after follow-up",
            );
            (handle, req.session_id.clone())
        }
        CallKind::Usage { .. }
        | CallKind::ArchiveChild(_)
        | CallKind::Peers
        | CallKind::SendPeer(_)
        | CallKind::Room(_)
        | CallKind::PeerHistory { .. }
        | CallKind::UserActionAdd(_)
        | CallKind::UserActionTick(_)
        | CallKind::Speak { .. }
        | CallKind::RelayReady
        | CallKind::RelayWait
        | CallKind::PreviewOpen { .. }
        | CallKind::PreviewClose { .. } => {
            unreachable!("these return above; they have no child to spawn or follow")
        }
    };
    let result =
        follow_child_with(&handle, &child_id, call.timeout, SILENT_CHILD_GRACE, call.proto, out)
            .await;
    let FollowResult::Finished(outcome) = result else {
        return annotate(follow_result_to_frame(result), &note);
    };
    if !should_nudge(&outcome) {
        return annotate(reply_frame(&outcome), &note);
    }
    let Some(target) = outcome.local_id.clone() else {
        return annotate(reply_frame(&outcome), &note);
    };
    drop(handle);
    annotate(nudge_once(server, machine_key, &call, &watch, &target, outcome, out).await, &note)
}

/// Send exactly one continuation prompt to a child that finished on a truncated
/// non-answer and follow that turn. Falls back to the original outcome when the
/// nudge cannot be relayed or the follow-up itself produces nothing.
async fn nudge_once(
    server: &ServerClient,
    machine_key: &str,
    call: &Call,
    watch: &std::sync::Arc<crate::childwatch::ChildWatch>,
    target: &str,
    original: crate::childwatch::ChildOutcome,
    out: &mut (impl AsyncWriteExt + Unpin),
) -> Value {
    let req =
        MessageChildRequest { session_id: target.to_owned(), prompt: NUDGE_PROMPT.to_owned() };
    let handle = watch.register_bound(target);
    if let Err(err) = server.message_child(machine_key, &call.session_id, &req).await {
        tracing::warn!(parent = %call.session_id, child = %target, %err, "CctuiAgent nudge failed");
        return reply_frame(&original);
    }
    tracing::info!(parent = %call.session_id, child = %target, "CctuiAgent nudging truncated child");
    match follow_child_with(&handle, target, call.timeout, SILENT_CHILD_GRACE, call.proto, out)
        .await
    {
        FollowResult::Finished(nudged) if nudged.final_text.is_some() || nudged.error.is_some() => {
            reply_frame(&nudged)
        }
        _ => reply_frame(&original),
    }
}

async fn handle_connection(stream: UnixStream, server: ServerClient, machine_key: String) {
    let (read_half, mut write_half) = stream.into_split();
    let mut lines = BufReader::new(read_half).lines();
    let Ok(Some(line)) = lines.next_line().await else { return };
    let frame = match parse_call(&line) {
        Ok(call) => run_call(&server, &machine_key, call, &mut write_half).await,
        Err(err) => json!({ "ok": false, "error": err }),
    };
    let _ = write_line(&mut write_half, &frame).await;
}

/// Serve the agent-tool socket until `shutdown`.
pub async fn serve(
    path: PathBuf,
    server: ServerClient,
    machine_key: String,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let listener = crate::runtime::bind_private_socket(&path)?;
    tracing::info!(socket = %path.display(), "CctuiAgent tool listener ready");
    loop {
        tokio::select! {
            () = shutdown.cancelled() => {
                let _ = std::fs::remove_file(&path);
                return Ok(());
            }
            accept = listener.accept() => {
                let (stream, _) = accept?;
                let server = server.clone();
                let machine_key = machine_key.clone();
                tokio::spawn(handle_connection(stream, server, machine_key));
            }
        }
    }
}

/// Whether the daemon can serve the tool at all: without a machine key there is
/// nobody to authorize a spawn against, so the listener stays off.
#[must_use]
pub fn is_available(machine_key: &str) -> bool {
    !machine_key.trim().is_empty()
}

/// Launch-key → real-session-id map for harnesses that mint their own id.
///
/// A codex thread id / opencode `ses_…` does not exist yet when the relay's
/// argv is baked, so those sessions carry their launch key as `--session`. The
/// server resolves a parent by `sessions.id`, so without this the call would
/// 404 against a key no session row uses.
static SESSION_ALIASES: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, String>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

/// Record that the session launched as `launch_key` really is `session_id`.
pub fn bind_session_alias(launch_key: &str, session_id: &str) {
    if launch_key.trim().is_empty() || session_id.trim().is_empty() || launch_key == session_id {
        return;
    }
    if let Ok(mut map) = SESSION_ALIASES.lock() {
        map.insert(launch_key.to_owned(), session_id.to_owned());
    }
}

/// Resolve a relay-supplied session id through [`bind_session_alias`]. An id
/// that was never aliased is already the real one and passes through.
#[must_use]
pub fn resolve_session_alias(id: &str) -> String {
    SESSION_ALIASES
        .lock()
        .ok()
        .and_then(|map| map.get(id).cloned())
        .unwrap_or_else(|| id.to_owned())
}

/// Path used when writing a session's MCP config, exposed for the launch path.
#[must_use]
pub fn socket_for_launch() -> &'static Path {
    static PATH: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    PATH.get_or_init(socket_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::childwatch::ChildOutcome;

    #[test]
    fn a_launch_key_alias_resolves_a_call_onto_the_real_parent_session() {
        bind_session_alias("launch-key-abc", "thread_0199real");
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "launch-key-abc",
            "args": { "prompt": "review this", "model": "gpt-5.6-sol", "adapter": "codex" },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        assert_eq!(
            call.session_id, "thread_0199real",
            "a codex/opencode child must be attributed to the thread id the server knows, \
             not the key baked into the relay argv"
        );
    }

    #[test]
    fn an_unaliased_session_id_passes_through() {
        assert_eq!(resolve_session_alias("never-bound"), "never-bound");
    }

    #[test]
    fn parses_a_full_spawn_call() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "parent-1",
            "timeout_secs": 120,
            "proto": 2,
            "args": {
                "adapter": "opencode",
                "prompt": "review the diff",
                "model": "accounts/fireworks/models/kimi-k3",
                "agent_profile": "cctui-reviewer",
                "budget_usd": 0.5,
                "cwd": "/workspace",
                "permission_mode": "auto",
                "name": "reviewer",
            },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        assert_eq!(call.session_id, "parent-1");
        assert_eq!(call.timeout, Duration::from_mins(2));
        assert_eq!(call.proto, 2);
        let CallKind::Spawn(req) = call.kind else { panic!("expected spawn") };
        assert_eq!(req.adapter, "opencode");
        assert_eq!(req.agent_profile.as_deref(), Some("cctui-reviewer"));
        assert_eq!(req.budget_usd, Some(0.5));
        assert_eq!(req.cwd.as_deref(), Some("/workspace"));
        assert_eq!(req.permission_mode, Some(cctui_proto::adapter::PermissionMode::Auto));
        assert_eq!(req.name.as_deref(), Some("reviewer"));
    }

    #[test]
    fn a_user_action_add_normalizes_its_kind_and_blocking_defaults() {
        let line = json!({
            "kind": "user_action_add",
            "session_id": "s1",
            "args": { "title": "  Approve PR #12  ", "kind": "nonsense" },
        })
        .to_string();
        let CallKind::UserActionAdd(body) = parse_call(&line).unwrap().kind else {
            panic!("expected an add")
        };
        assert_eq!(body["title"], json!("Approve PR #12"));
        assert_eq!(body["kind"], json!("action"));
        assert_eq!(body["blocking"], json!(false));
        assert!(body["detail"].is_null());
    }

    #[test]
    fn a_user_action_add_keeps_a_known_kind_and_a_blocking_flag() {
        let line = json!({
            "kind": "user_action_add",
            "session_id": "s1",
            "args": {
                "title": "Pick a layout",
                "kind": "decision",
                "blocking": true,
                "detail": " two options ",
            },
        })
        .to_string();
        let CallKind::UserActionAdd(body) = parse_call(&line).unwrap().kind else {
            panic!("expected an add")
        };
        assert_eq!(body["kind"], json!("decision"));
        assert_eq!(body["blocking"], json!(true));
        assert_eq!(body["detail"], json!("two options"));
    }

    #[test]
    fn a_tick_needs_an_id_and_a_terminal_status() {
        let ok = json!({
            "kind": "user_action_tick",
            "session_id": "s1",
            "args": { "id": "abc", "status": "dropped", "note": " never mind " },
        })
        .to_string();
        let CallKind::UserActionTick(body) = parse_call(&ok).unwrap().kind else {
            panic!("expected a tick")
        };
        assert_eq!(body["id"], json!("abc"));
        assert_eq!(body["status"], json!("dropped"));
        assert_eq!(body["note"], json!("never mind"));

        let no_id = json!({ "kind": "user_action_tick", "session_id": "s1", "args": {} });
        assert!(parse_call(&no_id.to_string()).is_err());
        let bad_status = json!({
            "kind": "user_action_tick",
            "session_id": "s1",
            "args": { "id": "abc", "status": "open" },
        });
        let err = parse_call(&bad_status.to_string()).unwrap_err();
        assert!(err.contains("done"), "{err}");
    }

    #[test]
    fn the_rendered_list_names_open_blocking_items_and_who_resolved_the_rest() {
        let payload = json!({
            "list": {
                "session_id": "s1",
                "items": [
                    {
                        "id": "id-1",
                        "title": "Approve PR #12",
                        "kind": "decision",
                        "blocking": true,
                        "status": "open",
                    },
                    {
                        "id": "id-2",
                        "title": "Run gcloud auth login",
                        "kind": "action",
                        "status": "done",
                        "resolved_by": "user",
                        "note": "token received",
                    },
                ],
            },
            "added": "id-1",
        });
        let text = render_user_action_result(&payload);
        assert!(text.contains("user actions (1 open of 2):"), "{text}");
        assert!(text.contains("[ ] BLOCKING Approve PR #12 (decision, id id-1)"), "{text}");
        assert!(text.contains("[done by user] Run gcloud auth login (id id-2)"), "{text}");
        assert!(text.contains("token received"), "{text}");
        assert!(text.contains("item id: id-1"), "{text}");
    }

    #[test]
    fn a_rejection_is_rendered_with_the_list_so_the_model_sees_the_true_state() {
        let text = render_user_action_result(&json!({
            "error": "title is required and was empty",
            "list": { "session_id": "s1", "items": [] },
        }));
        assert!(text.starts_with("rejected: title is required"), "{text}");
        assert!(text.contains("the user action list is empty"), "{text}");
    }

    #[test]
    fn the_relay_readiness_ops_parse_without_any_args() {
        let ready = json!({ "kind": "relay_ready", "session_id": "s1" }).to_string();
        assert!(matches!(parse_call(&ready).unwrap().kind, CallKind::RelayReady));
        let wait = json!({ "kind": "relay_wait", "session_id": "s1", "timeout_secs": 8 });
        let call = parse_call(&wait.to_string()).unwrap();
        assert!(matches!(call.kind, CallKind::RelayWait));
        assert_eq!(call.timeout, Duration::from_secs(8));
    }

    #[test]
    fn a_session_id_arg_turns_the_call_into_a_follow_up() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "parent-1",
            "args": { "session_id": "child-9", "prompt": "and check the tests" },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        let CallKind::Message(req) = call.kind else { panic!("expected message") };
        assert_eq!(req.session_id, "child-9");
        assert_eq!(req.prompt, "and check the tests");
    }

    #[test]
    fn proto_defaults_to_1_for_old_relays() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "adapter": "codex", "prompt": "go", "model": "gpt-5.6-sol" },
        })
        .to_string();
        assert_eq!(parse_call(&line).unwrap().proto, 1);
    }

    #[test]
    fn blank_optional_args_are_dropped_not_forwarded_empty() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "adapter": "codex", "prompt": "go", "model": "gpt-5.6-sol", "cwd": "",
                      "permission_mode": "notamode" },
        })
        .to_string();
        let call = parse_call(&line).unwrap();
        let CallKind::Spawn(req) = call.kind else { panic!("expected spawn") };
        assert!(req.cwd.is_none());
        assert!(req.permission_mode.is_none());
    }

    #[test]
    fn a_spawn_without_a_model_is_rejected_and_the_error_says_where_to_find_one() {
        for args in [
            json!({ "adapter": "claude-code", "prompt": "go" }),
            json!({ "adapter": "claude-code", "prompt": "go", "model": "   " }),
            json!({ "adapter": "claude-code", "prompt": "go", "model": "" }),
        ] {
            let line =
                json!({ "kind": "spawn_agent", "session_id": "p", "args": args }).to_string();
            let Err(err) = parse_call(&line) else { panic!("a spawn without a model must fail") };
            assert!(err.contains("model is required"), "{err}");
            assert!(err.contains("never falls back"), "{err}");
            assert!(err.contains("per_model"), "{err}");
        }
    }

    #[test]
    fn the_missing_model_error_names_no_concrete_model_id() {
        let err = missing_model_error();
        for stale in ["claude-opus", "claude-sonnet", "claude-haiku", "claude-fable", "gpt-5"] {
            assert!(!err.contains(stale), "compiled-in model id {stale:?} in: {err}");
        }
    }

    #[test]
    fn a_model_id_absent_from_any_list_still_parses() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "adapter": "claude-code", "prompt": "go", "model": "claude-opus-9-9[1m]" },
        })
        .to_string();
        let CallKind::Spawn(req) = parse_call(&line).expect("parses").kind else {
            panic!("expected a spawn")
        };
        assert_eq!(req.model.as_deref(), Some("claude-opus-9-9[1m]"));
    }

    #[test]
    fn a_spawn_with_a_model_still_parses_unchanged() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "adapter": "claude", "prompt": "go", "model": " claude-opus-5[1m] " },
        })
        .to_string();
        let CallKind::Spawn(req) = parse_call(&line).unwrap().kind else {
            panic!("expected spawn")
        };
        assert_eq!(req.model.as_deref(), Some("claude-opus-5[1m]"));
        assert_eq!(req.adapter, "claude-code");
    }

    #[test]
    fn a_follow_up_needs_no_model() {
        let line = json!({
            "kind": "spawn_agent",
            "session_id": "p",
            "args": { "session_id": "child-9", "prompt": "carry on" },
        })
        .to_string();
        assert!(matches!(parse_call(&line).unwrap().kind, CallKind::Message(_)));
    }

    #[test]
    fn every_frame_echoes_the_model_and_the_follow_window() {
        let spawn = CallKind::Spawn(SpawnChildRequest {
            adapter: "claude-code".to_owned(),
            prompt: "go".to_owned(),
            model: Some("claude-opus-5[1m]".to_owned()),
            agent_profile: None,
            budget_usd: None,
            cwd: None,
            permission_mode: None,
            name: None,
        });
        let note = dispatch_note(&spawn, Duration::from_hours(2));
        assert!(note.contains("spawned on model claude-opus-5[1m]"), "{note}");
        assert!(note.contains("adapter claude-code"), "{note}");
        assert!(note.contains("follow window 7200s"), "{note}");

        let ok = annotate(json!({ "ok": true, "result": "all done" }), &note);
        let text = ok["result"].as_str().unwrap();
        assert!(text.starts_with("all done"));
        assert!(text.contains("claude-opus-5[1m]"), "{text}");

        let failed = annotate(json!({ "ok": false, "error": "child agent failed" }), &note);
        assert!(failed["error"].as_str().unwrap().contains("claude-opus-5[1m]"));

        let follow = dispatch_note(
            &CallKind::Message(MessageChildRequest {
                session_id: "child-9".to_owned(),
                prompt: "carry on".to_owned(),
            }),
            Duration::from_mins(30),
        );
        assert!(follow.contains("follow-up to child child-9"), "{follow}");
        assert!(follow.contains("follow window 1800s"), "{follow}");
        assert!(follow.contains("`model` is ignored here"), "{follow}");
    }

    #[test]
    fn a_call_without_a_prompt_or_session_is_rejected() {
        let no_prompt =
            json!({ "kind": "spawn_agent", "session_id": "p", "args": { "adapter": "codex" } });
        assert!(parse_call(&no_prompt.to_string()).is_err());
        let no_session = json!({ "kind": "spawn_agent", "args": { "prompt": "x" } });
        assert!(parse_call(&no_session.to_string()).is_err());
        assert!(parse_call("not json").is_err());
        assert!(parse_call(&json!({ "kind": "other" }).to_string()).is_err());
    }

    #[test]
    fn model_spellings_normalize_to_adapter_ids() {
        assert_eq!(normalize_adapter("claude_code"), "claude-code");
        assert_eq!(normalize_adapter("Claude"), "claude-code");
        assert_eq!(normalize_adapter("codex-cli"), "codex");
        assert_eq!(normalize_adapter(" opencode "), "opencode");
        assert_eq!(normalize_adapter("Gemini_CLI"), "gemini");
    }

    #[test]
    fn reply_frames_distinguish_success_failure_and_silence() {
        let out = |text: Option<&str>, err: Option<&str>| ChildOutcome {
            final_text: text.map(str::to_owned),
            error: err.map(str::to_owned),
            local_id: Some("child-7".into()),
            tail_is_thinking: false,
        };
        let ok = reply_frame(&out(Some("verdict: ship"), None));
        assert_eq!(ok["ok"], json!(true));
        let text = ok["result"].as_str().unwrap();
        assert!(text.starts_with("verdict: ship"));
        assert!(text.contains("child-7"), "reply must carry the child id: {text}");

        let failed = reply_frame(&out(None, Some("crashed")));
        assert_eq!(failed["ok"], json!(false));
        assert!(failed["error"].as_str().unwrap().contains("crashed"));

        let partial = reply_frame(&out(Some("got halfway"), Some("killed")));
        assert_eq!(partial["ok"], json!(false));
        let text = partial["error"].as_str().unwrap();
        assert!(text.contains("killed") && text.contains("got halfway"));

        let silent = reply_frame(&out(None, None));
        assert_eq!(silent["ok"], json!(true));
        assert!(silent["result"].as_str().unwrap().contains("without producing any output"));
    }

    #[tokio::test]
    async fn follow_child_streams_progress_then_final_result() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        let observer = watch.clone();
        let feeder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            observer.observe(&cctui_proto::adapter::AdapterEvent::Message {
                local_id: "child-1".into(),
                payload: json!({ "role": "assistant", "text": "all done" }),
                turn_id: None,
            });
            observer.observe(&cctui_proto::adapter::AdapterEvent::SessionEnded {
                local_id: "child-1".into(),
                reason: cctui_proto::adapter::EndReason::Completed,
            });
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_result_to_frame(
            follow_child_with(
                &handle,
                "child-1",
                Duration::from_secs(10),
                SILENT_CHILD_GRACE,
                2,
                &mut out,
            )
            .await,
        );
        feeder.await.unwrap();
        assert_eq!(frame["ok"], json!(true));
        assert!(frame["result"].as_str().unwrap().starts_with("all done"));
    }

    #[tokio::test]
    async fn follow_keeps_waiting_through_narration_and_returns_the_real_answer() {
        // acceptance: the live failure — narration was returned while the child
        // kept working. The follow must hold until the model says end_turn.
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        let observer = watch.clone();
        let feeder = tokio::spawn(async move {
            let narrate = |text: &str| cctui_proto::adapter::AdapterEvent::Message {
                local_id: "child-1".into(),
                payload: json!({ "role": "assistant", "text": text, "stop_reason": "tool_use" }),
                turn_id: None,
            };
            tokio::time::sleep(Duration::from_millis(30)).await;
            observer.observe(&narrate("Sanity-checking the final diffs by eye."));
            tokio::time::sleep(Duration::from_millis(30)).await;
            observer.observe(&narrate("One more sweep of the remaining unchecked writes."));
            tokio::time::sleep(Duration::from_millis(30)).await;
            observer.observe(&cctui_proto::adapter::AdapterEvent::ToolUse {
                local_id: "child-1".into(),
                payload: json!({ "tool": "Bash" }),
            });
            tokio::time::sleep(Duration::from_millis(30)).await;
            observer.observe(&cctui_proto::adapter::AdapterEvent::Message {
                local_id: "child-1".into(),
                payload: json!({
                    "role": "assistant",
                    "text": "done: both commits are in",
                    "stop_reason": "end_turn",
                }),
                turn_id: None,
            });
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_result_to_frame(
            follow_child_with(
                &handle,
                "child-1",
                Duration::from_secs(10),
                SILENT_CHILD_GRACE,
                2,
                &mut out,
            )
            .await,
        );
        feeder.await.unwrap();
        assert_eq!(frame["ok"], json!(true));
        let text = frame["result"].as_str().unwrap();
        assert!(text.starts_with("done: both commits are in"), "{text}");
        assert!(!text.contains("Sanity-checking"), "narration must never be the answer: {text}");
    }

    #[tokio::test]
    async fn follow_returns_the_answer_of_a_child_that_self_reports_blocked() {
        // acceptance: the follow window must not be waited out by a child whose
        // answer ended in a question, which leaves its job state `blocked`.
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        let observer = watch.clone();
        let feeder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            observer.observe(&cctui_proto::adapter::AdapterEvent::Message {
                local_id: "child-1".into(),
                payload: json!({ "role": "assistant", "text": "which skill did you mean?" }),
                turn_id: None,
            });
            observer.observe(&cctui_proto::adapter::AdapterEvent::Status {
                local_id: "child-1".into(),
                tempo: Some("blocked".into()),
                state: Some("blocked".into()),
                detail: Some("clarify search".into()),
                activity: None,
                name: None,
                intent: None,
                model: None,
                effort: None,
                permission_mode: None,
                children: Vec::new(),
            });
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_result_to_frame(
            follow_child_with(
                &handle,
                "child-1",
                Duration::from_secs(10),
                SILENT_CHILD_GRACE,
                2,
                &mut out,
            )
            .await,
        );
        feeder.await.unwrap();
        assert_eq!(frame["ok"], json!(true));
        assert!(frame["result"].as_str().unwrap().starts_with("which skill did you mean?"));
    }

    #[tokio::test]
    async fn follow_child_times_out_with_a_follow_up_hint() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionStarted {
            local_id: "child-1".into(),
            meta: cctui_proto::adapter::SessionMeta::default(),
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_result_to_frame(
            follow_child_with(
                &handle,
                "child-1",
                Duration::from_millis(10),
                SILENT_CHILD_GRACE,
                1,
                &mut out,
            )
            .await,
        );
        assert_eq!(frame["ok"], json!(false));
        let text = frame["error"].as_str().unwrap();
        assert!(text.contains("NOT A CRASH"), "{text}");
        assert!(text.contains("still running"), "{text}");
        assert!(text.contains("on disk"), "{text}");
        assert!(text.contains("session_id"), "{text}");
        assert!(out.is_empty(), "proto 1 must never receive progress frames");
    }

    #[tokio::test]
    async fn a_silent_child_fails_fast_instead_of_waiting_out_the_timeout() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionStarted {
            local_id: "child-1".into(),
            meta: cctui_proto::adapter::SessionMeta::default(),
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_result_to_frame(
            follow_child_with(
                &handle,
                "child-1",
                Duration::from_mins(30),
                Duration::from_millis(10),
                2,
                &mut out,
            )
            .await,
        );
        assert_eq!(frame["ok"], json!(false));
        let text = frame["error"].as_str().unwrap();
        assert!(text.contains("no activity"), "{text}");
        assert!(text.contains("died at startup"), "{text}");
    }

    #[tokio::test]
    async fn a_child_that_showed_activity_is_never_failed_fast() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionStarted {
            local_id: "child-1".into(),
            meta: cctui_proto::adapter::SessionMeta::default(),
        });
        watch.observe(&cctui_proto::adapter::AdapterEvent::ToolUse {
            local_id: "child-1".into(),
            payload: json!({ "tool": "Bash" }),
        });
        let mut out: Vec<u8> = Vec::new();
        let frame = follow_result_to_frame(
            follow_child_with(
                &handle,
                "child-1",
                Duration::from_millis(20),
                Duration::from_millis(10),
                1,
                &mut out,
            )
            .await,
        );
        let text = frame["error"].as_str().unwrap();
        assert!(
            text.contains("still running"),
            "a working child must hit the timeout path: {text}"
        );
    }

    #[tokio::test]
    async fn a_crashed_child_returns_before_the_timeout() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        let observer = watch.clone();
        let feeder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            observer.observe(&cctui_proto::adapter::AdapterEvent::SessionEnded {
                local_id: "child-1".into(),
                reason: cctui_proto::adapter::EndReason::Crashed {
                    detail: "gateway rejected the first model call".into(),
                },
            });
        });
        let mut out: Vec<u8> = Vec::new();
        let started = Instant::now();
        let frame = follow_result_to_frame(
            follow_child_with(
                &handle,
                "child-1",
                Duration::from_mins(30),
                Duration::from_mins(30),
                2,
                &mut out,
            )
            .await,
        );
        feeder.await.unwrap();
        assert!(started.elapsed() < Duration::from_secs(30), "must not wait out the timeout");
        assert_eq!(frame["ok"], json!(false));
        assert!(frame["error"].as_str().unwrap().contains("gateway rejected"));
    }

    #[test]
    fn truncated_non_answers_are_detected() {
        assert!(looks_truncated(None));
        assert!(looks_truncated(Some("   \n  ")));
        assert!(looks_truncated(Some("Reviewing the diff.\n\nNow I need to verify key claims:")));
        assert!(looks_truncated(Some("Here is my plan.\n1.")));
        assert!(looks_truncated(Some("First steps:\n-")));
        assert!(looks_truncated(Some("Considering the options:\n2)")));
    }

    #[test]
    fn real_deliverables_are_not_truncated() {
        assert!(!looks_truncated(Some("VERDICT: approve")));
        assert!(!looks_truncated(Some(
            "Findings:\n1. off-by-one at line 4\n2. missing await\n\nOverall: ship after fixes."
        )));
        assert!(!looks_truncated(Some("No issues found.")));
        assert!(!looks_truncated(Some("Line 4: the guard is inverted.")));
    }

    #[test]
    fn only_clean_truncated_turns_are_nudged_never_crashes() {
        let outcome = |text: Option<&str>, err: Option<&str>| ChildOutcome {
            final_text: text.map(str::to_owned),
            error: err.map(str::to_owned),
            local_id: Some("child-1".into()),
            tail_is_thinking: false,
        };
        assert!(should_nudge(&outcome(Some("Now I need to verify key claims:"), None)));
        assert!(should_nudge(&outcome(None, None)));
        assert!(!should_nudge(&outcome(Some("VERDICT: approve"), None)));
        assert!(!should_nudge(&outcome(Some("Now I need to verify:"), Some("crashed"))));
        assert!(!should_nudge(&outcome(None, Some("gateway rejected"))));
        assert!(!should_nudge(&outcome(Some("Findings: none, ship it."), None)));
    }

    #[test]
    fn a_thinking_tail_nudges_even_when_the_held_text_reads_complete() {
        let outcome = |thinking: bool, err: Option<&str>| ChildOutcome {
            final_text: Some("Verified the fix, all tests pass.".to_owned()),
            error: err.map(str::to_owned),
            local_id: Some("child-1".into()),
            tail_is_thinking: thinking,
        };
        assert!(should_nudge(&outcome(true, None)));
        assert!(!should_nudge(&outcome(false, None)));
        assert!(!should_nudge(&outcome(true, Some("crashed"))));
    }

    #[test]
    fn narration_then_thinking_end_nudges_but_a_final_text_after_thinking_does_not() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::Message {
            local_id: "child-1".into(),
            payload: json!({ "role": "assistant", "text": "Checked the diff, looks clean." }),
            turn_id: None,
        });
        watch.observe(&cctui_proto::adapter::AdapterEvent::Message {
            local_id: "child-1".into(),
            payload: json!({ "role": "assistant_thinking", "text": "now let me verify" }),
            turn_id: None,
        });
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionEnded {
            local_id: "child-1".into(),
            reason: cctui_proto::adapter::EndReason::Completed,
        });
        let snap = handle.snapshot().unwrap();
        let Assessment::Finished(outcome) = snap.assess(Instant::now()) else {
            panic!("ended child must be finished");
        };
        assert!(outcome.tail_is_thinking);
        assert!(should_nudge(&outcome), "stale narration held as final text must nudge");

        let handle = watch.register("child-2");
        watch.observe(&cctui_proto::adapter::AdapterEvent::Message {
            local_id: "child-2".into(),
            payload: json!({ "role": "assistant_thinking", "text": "planning" }),
            turn_id: None,
        });
        watch.observe(&cctui_proto::adapter::AdapterEvent::Message {
            local_id: "child-2".into(),
            payload: json!({ "role": "assistant", "text": "VERDICT: approve" }),
            turn_id: None,
        });
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionEnded {
            local_id: "child-2".into(),
            reason: cctui_proto::adapter::EndReason::Completed,
        });
        let snap = handle.snapshot().unwrap();
        let Assessment::Finished(outcome) = snap.assess(Instant::now()) else {
            panic!("ended child must be finished");
        };
        assert!(!outcome.tail_is_thinking);
        assert!(!should_nudge(&outcome));
    }

    #[test]
    fn a_killed_child_is_never_nudged() {
        let watch = std::sync::Arc::new(crate::childwatch::ChildWatch::default());
        let handle = watch.register("child-1");
        watch.observe(&cctui_proto::adapter::AdapterEvent::Message {
            local_id: "child-1".into(),
            payload: json!({ "role": "assistant", "text": "Now I need to verify key claims:" }),
            turn_id: None,
        });
        watch.observe(&cctui_proto::adapter::AdapterEvent::SessionEnded {
            local_id: "child-1".into(),
            reason: cctui_proto::adapter::EndReason::Killed,
        });
        let snap = handle.snapshot().unwrap();
        let Assessment::Finished(outcome) = snap.assess(Instant::now()) else {
            panic!("killed child must be finished");
        };
        assert!(outcome.error.as_deref().unwrap().contains("killed"));
        assert!(
            !should_nudge(&outcome),
            "a killed child must never be nudged even on truncated text"
        );
    }

    #[test]
    fn a_speak_call_needs_text_within_the_cap_and_keeps_the_voice() {
        let ok = json!({ "kind": "speak", "session_id": "p", "args": { "text": " hi ", "voice": "af_bella" } });
        let CallKind::Speak { text, voice } = parse_call(&ok.to_string()).unwrap().kind else {
            panic!("expected speak")
        };
        assert_eq!(text, "hi");
        assert_eq!(voice.as_deref(), Some("af_bella"));

        let empty = json!({ "kind": "speak", "session_id": "p", "args": { "text": "  " } });
        assert!(parse_call(&empty.to_string()).unwrap_err().contains("text is required"));

        let long = "a".repeat(crate::mcp::SPEAK_MAX_CHARS + 1);
        let too_long = json!({ "kind": "speak", "session_id": "p", "args": { "text": long } });
        assert!(parse_call(&too_long.to_string()).unwrap_err().contains("too long"));
    }

    #[test]
    fn a_usage_call_parses_without_a_prompt_and_carries_the_optional_model() {
        let bare = json!({ "kind": "usage", "session_id": "p", "args": {} }).to_string();
        let call = parse_call(&bare).unwrap();
        assert_eq!(call.session_id, "p");
        let CallKind::Usage { model } = call.kind else { panic!("expected usage") };
        assert!(model.is_none());

        let with_model =
            json!({ "kind": "usage", "session_id": "p", "args": { "model": " claude-opus-5 " } })
                .to_string();
        let CallKind::Usage { model } = parse_call(&with_model).unwrap().kind else {
            panic!("expected usage")
        };
        assert_eq!(model.as_deref(), Some("claude-opus-5"));

        let blank =
            json!({ "kind": "usage", "session_id": "p", "args": { "model": "  " } }).to_string();
        let CallKind::Usage { model } = parse_call(&blank).unwrap().kind else {
            panic!("expected usage")
        };
        assert!(model.is_none(), "a blank model must not be forwarded");
    }

    #[test]
    fn a_usage_call_still_needs_a_session_and_carries_no_dispatch_note() {
        assert!(parse_call(&json!({ "kind": "usage", "args": {} }).to_string()).is_err());
        assert_eq!(
            dispatch_note(&CallKind::Usage { model: None }, Duration::from_secs(30)),
            "",
            "a usage call spawns nothing, so there is no model or follow window to echo"
        );
    }

    #[test]
    fn a_spawn_call_still_requires_its_prompt() {
        let no_prompt = json!({ "kind": "spawn_agent", "session_id": "p", "args": {} });
        assert_eq!(parse_call(&no_prompt.to_string()).unwrap_err(), "prompt is required");
    }

    #[test]
    fn the_usage_line_names_the_account_windows_budget_and_blocked_models() {
        let resets = (chrono::Utc::now() + chrono::Duration::minutes(130)).to_rfc3339();
        let line = render_usage(&json!({
            "account": { "name": "dorsk-main", "emoji": "🐧" },
            "windows": [
                { "key": "session", "label": "5h", "utilization": 46.0, "resets_at": resets },
                { "key": "weekly_all", "label": "weekly", "utilization": 71.0 },
            ],
            "caps": { "session_usd": { "cap_usd": 20.0 } },
            "spend": { "session_usd": 3.42 },
            "per_model": {
                "claude-fable-5-1": {
                    "allow": false, "retry_after_secs": 5400, "key": "weekly_model:fable",
                },
                "claude-opus-5": { "allow": true },
            },
            "stale": false,
        }));
        assert!(line.contains("🐧dorsk-main"), "{line}");
        assert!(line.contains("5h 46%"), "{line}");
        assert!(line.contains("resets in 2h"), "{line}");
        assert!(line.contains("weekly 71%"), "{line}");
        assert!(line.contains("budget $3.42/$20"), "{line}");
        assert!(line.contains("claude-fable-5-1 weekly_model:fable BLOCKED for 1h30"), "{line}");
        assert!(line.contains("claude-opus-5 ok"), "{line}");
        assert!(!line.contains("stale"), "{line}");
    }

    #[test]
    fn a_stale_or_empty_limits_payload_still_renders_something_readable() {
        let stale = render_usage(&json!({
            "account": { "name": "shared", "provider": "anthropic" },
            "windows": [],
            "decision": { "allow": true },
            "stale": true,
        }));
        assert!(stale.contains("shared"), "{stale}");
        assert!(stale.contains("usage cache stale"), "{stale}");
        assert_eq!(render_usage(&json!({})), "no usage information is available for this session");
    }

    #[test]
    fn the_usage_line_reports_child_slots() {
        let capped = render_usage(&json!({ "children": { "used": 3, "max": 16 } }));
        assert_eq!(capped, "children 3/16");
        let uncapped = render_usage(&json!({ "children": { "used": 2 } }));
        assert_eq!(uncapped, "children 2");
    }

    #[test]
    fn an_archive_call_needs_a_target_and_parses_into_its_own_kind() {
        let line =
            json!({ "kind": "archive_child", "session_id": "p", "args": { "session_id": " c1 " } })
                .to_string();
        let call = parse_call(&line).unwrap();
        let CallKind::ArchiveChild(req) = call.kind else { panic!("expected archive_child") };
        assert_eq!(req.session_id, "c1");
        assert!(dispatch_note(&CallKind::ArchiveChild(req), Duration::from_secs(30)).is_empty());

        let missing = json!({ "kind": "archive_child", "session_id": "p", "args": {} }).to_string();
        assert!(parse_call(&missing).unwrap_err().contains("session_id is required"));
    }

    #[test]
    fn an_archive_result_names_the_rows_and_the_slots_left() {
        let resp = ArchiveChildResponse {
            archived: vec!["c1".into(), "c1-sub".into()],
            children_used: 15,
            max_children: Some(16),
        };
        assert_eq!(render_archived(&resp), "archived c1, c1-sub · 15/16 child slots used");
        let none = ArchiveChildResponse { archived: vec![], children_used: 4, max_children: None };
        assert_eq!(render_archived(&none), "nothing archived · 4 child slots used");
    }

    #[test]
    fn a_whole_dollar_cap_drops_its_cents_and_a_fractional_one_keeps_them() {
        assert_eq!(money(20.0), "20");
        assert_eq!(money(5.0), "5");
        assert_eq!(money(0.0), "0");
        assert_eq!(money(7.5), "7.50");
        assert_eq!(money(0.25), "0.25");
        assert_eq!(money(3.42), "3.42");
    }

    #[test]
    fn a_fractional_budget_cap_renders_with_cents() {
        let line = render_usage(&json!({
            "caps": { "session_usd": { "cap_usd": 0.5 } },
            "spend": { "session_usd": 0.13 },
        }));
        assert!(line.contains("budget $0.13/$0.50"), "{line}");
    }

    #[test]
    fn durations_render_compactly() {
        assert_eq!(human_duration(7830), "2h10");
        assert_eq!(human_duration(5400), "1h30");
        assert_eq!(human_duration(3600), "1h00");
        assert_eq!(human_duration(90), "1m");
        assert_eq!(human_duration(30), "30s");
        assert_eq!(human_duration(0), "now");
        assert_eq!(human_duration(-5), "now");
    }

    #[test]
    fn a_peers_call_needs_nothing_but_the_session() {
        let line = json!({ "kind": "peers", "session_id": "s1", "args": {} }).to_string();
        let call = parse_call(&line).unwrap();
        assert_eq!(call.session_id, "s1");
        assert!(matches!(call.kind, CallKind::Peers));
        assert_eq!(dispatch_note(&CallKind::Peers, Duration::from_secs(30)), "");
        assert!(parse_call(&json!({ "kind": "peers", "args": {} }).to_string()).is_err());
    }

    #[test]
    fn a_send_call_carries_the_target_and_the_trimmed_message() {
        let line = json!({
            "kind": "send_peer",
            "session_id": "s1",
            "args": { "session_id": " peer-9 ", "message": "  check the tests  " },
        })
        .to_string();
        let CallKind::SendPeer(req) = parse_call(&line).unwrap().kind else {
            panic!("expected send_peer")
        };
        assert_eq!(req.session_id, "peer-9");
        assert_eq!(req.message, "check the tests");
    }

    #[test]
    fn a_send_call_without_a_target_or_a_message_is_rejected_by_name() {
        let no_target = json!({ "kind": "send_peer", "session_id": "s1",
                                "args": { "message": "hi" } });
        assert!(parse_call(&no_target.to_string()).unwrap_err().contains("session_id is required"));
        let no_message = json!({ "kind": "send_peer", "session_id": "s1",
                                 "args": { "session_id": "peer-9", "message": "   " } });
        assert!(parse_call(&no_message.to_string()).unwrap_err().contains("message is required"));
    }

    /// The launch-key alias applies to peer calls too: a codex or opencode
    /// session must be attributed to the thread id the server knows.
    #[test]
    fn a_peer_call_resolves_the_launch_key_alias() {
        bind_session_alias("peer-launch-key", "ses_realpeer");
        let line =
            json!({ "kind": "peers", "session_id": "peer-launch-key", "args": {} }).to_string();
        assert_eq!(parse_call(&line).unwrap().session_id, "ses_realpeer");
    }

    #[test]
    fn a_history_call_becomes_the_query_string_the_route_expects() {
        let line = json!({
            "kind": "peer_history",
            "session_id": "s1",
            "args": {
                "session_id": "peer-9", "before": 4_242, "limit": 50,
                "roles": ["user", " assistant ", ""], "format": "MarkDown",
            },
        })
        .to_string();
        let CallKind::PeerHistory { query } = parse_call(&line).unwrap().kind else {
            panic!("expected peer_history")
        };
        let get = |k: &str| {
            query.iter().find(|(key, _)| *key == k).map(|(_, v)| v.clone()).unwrap_or_default()
        };
        assert_eq!(get("session_id"), "peer-9");
        assert_eq!(get("before"), "4242");
        assert_eq!(get("limit"), "50");
        assert_eq!(get("roles"), "user,assistant");
        assert_eq!(get("format"), "markdown");
        assert!(query.iter().all(|(k, _)| *k != "after"), "an absent knob must not be sent");
    }

    #[test]
    fn a_history_call_with_no_knobs_sends_only_the_target() {
        let line = json!({
            "kind": "peer_history", "session_id": "s1", "args": { "session_id": "peer-9" },
        })
        .to_string();
        let CallKind::PeerHistory { query, .. } = parse_call(&line).unwrap().kind else {
            panic!("expected peer_history")
        };
        assert_eq!(query, vec![("session_id", "peer-9".to_owned())]);
    }

    #[test]
    fn roles_accept_an_array_or_a_comma_string_and_nothing_else() {
        assert_eq!(roles_arg(&json!({ "roles": ["user", "tool"] })).as_deref(), Some("user,tool"));
        assert_eq!(roles_arg(&json!({ "roles": " user , tool " })).as_deref(), Some("user , tool"));
        assert!(roles_arg(&json!({ "roles": [] })).is_none());
        assert!(roles_arg(&json!({ "roles": "  " })).is_none());
        assert!(roles_arg(&json!({ "roles": 7 })).is_none());
        assert!(roles_arg(&json!({})).is_none());
    }

    #[test]
    fn the_roster_renders_as_one_line_per_peer_and_says_so_when_empty() {
        let empty = render_peers(&json!({ "peers": [] }));
        assert!(empty.contains("no addressable peers"), "{empty}");
        assert_eq!(render_peers(&json!({})), empty, "a malformed reply reads as empty");

        let out = render_peers(&json!({ "peers": [
            { "session_id": "p1", "name": "lane a", "adapter": "codex", "machine": "box-b",
              "state": "live", "relation": "sibling" },
            { "session_id": "p2", "name": "  ", "adapter": "claude-code", "machine": "box-a",
              "state": "archived", "relation": "parent" },
        ] }));
        assert!(out.starts_with("2 addressable peer(s):"), "{out}");
        assert!(out.contains("- p1 [sibling] lane a · codex on box-b · live"), "{out}");
        assert!(out.contains("- p2 [parent] (unnamed) · claude-code on box-a · archived"), "{out}");
    }

    #[test]
    fn a_history_reply_returns_the_markdown_with_a_cursor_note() {
        let out = render_history(&json!({
            "markdown": "# lane a\n\n**user**\n\ngo", "events": 12,
            "first_seq": 900, "last_seq": 950, "truncated": true,
        }));
        assert!(out.starts_with("# lane a"), "{out}");
        assert!(out.contains("[12 event(s)"), "{out}");
        assert!(out.contains("oldest seq 900"), "{out}");
        assert!(out.contains("before=900"), "{out}");

        let whole = render_history(&json!({
            "markdown": "x", "events": 2, "first_seq": 1, "truncated": false,
        }));
        assert!(!whole.contains("before="), "a complete page must not suggest paging: {whole}");
    }

    #[test]
    fn a_json_history_reply_falls_back_to_the_raw_items() {
        let out = render_history(&json!({
            "events": 1, "items": [{ "seq": 4, "role": "user", "payload": { "type": "text" } }],
        }));
        assert!(out.contains("\"role\": \"user\""), "{out}");
        assert!(out.contains("[1 event(s)]"), "{out}");
    }

    #[test]
    fn a_room_call_parses_its_action_and_normalizes_the_spelling() {
        let line = json!({
            "kind": "room",
            "session_id": "s1",
            "args": { "action": " PoSt ", "message": " the gate is green ", "room_id": " r-1 " },
        })
        .to_string();
        let CallKind::Room(req) = parse_call(&line).unwrap().kind else { panic!("expected room") };
        assert_eq!(req.action, "post");
        assert_eq!(req.message.as_deref(), Some("the gate is green"));
        assert_eq!(req.room_id.as_deref(), Some("r-1"));
        assert_eq!(dispatch_note(&CallKind::Room(req), Duration::from_secs(30)), "");
    }

    #[test]
    fn peek_and_members_need_no_message_but_post_does() {
        for action in ["peek", "members"] {
            let line = json!({ "kind": "room", "session_id": "s1", "args": { "action": action } })
                .to_string();
            let CallKind::Room(req) = parse_call(&line).unwrap().kind else {
                panic!("expected room")
            };
            assert_eq!(req.action, action);
            assert!(req.message.is_none());
            assert!(req.room_id.is_none(), "an omitted room means the caller's only room");
        }
        let no_message =
            json!({ "kind": "room", "session_id": "s1", "args": { "action": "post" } });
        assert!(
            parse_call(&no_message.to_string()).unwrap_err().contains("message is required"),
            "a post with nothing to say must be rejected before it reaches the server"
        );
        let no_action = json!({ "kind": "room", "session_id": "s1", "args": {} });
        assert!(parse_call(&no_action.to_string()).unwrap_err().contains("action is required"));
    }

    /// A post reply must not read like a request/response: the model has to know
    /// no answer is coming back through the call.
    /// A broadcast is best effort, so the reply must name what did NOT land: a
    /// silent skip reads as a delivery, and the agent then waits on a session
    /// that never heard it.
    #[test]
    fn a_post_reply_names_the_reach_and_every_session_it_missed() {
        let out = render_room(
            "post",
            "me",
            &json!({
                "room": "wave 23", "room_id": "r-1", "seq": 7, "delivered": 1,
                "receipts": [
                    { "session_id": "b", "label": "lane b (codex on box-b)",
                      "outcome": "delivered" },
                    { "session_id": "c", "label": "lane c (claude-code on box-a)",
                      "outcome": "archived" },
                    { "session_id": "d", "label": "lane d (codex on box-c)",
                      "outcome": "offline" },
                ],
            }),
        );
        assert!(out.contains("posted to wave 23 as #7"), "{out}");
        assert!(out.contains("delivered to 1 of 3 other session(s)"), "{out}");
        assert!(out.contains("nothing comes back through this call"), "{out}");
        assert!(out.contains("Not delivered:"), "{out}");
        assert!(out.contains("lane c (claude-code on box-a) (archived)"), "{out}");
        assert!(out.contains("lane d (codex on box-c) (offline)"), "{out}");
        assert!(!out.contains("lane b"), "a delivered session is not listed as missed: {out}");
    }

    #[test]
    fn a_fully_delivered_post_lists_nothing_as_missed() {
        let out = render_room(
            "post",
            "me",
            &json!({
                "room": "wave 23", "seq": 1, "delivered": 1,
                "receipts": [{ "session_id": "b", "label": "lane b", "outcome": "delivered" }],
            }),
        );
        assert!(out.contains("delivered to 1 of 1 other session(s)"), "{out}");
        assert!(!out.contains("Not delivered"), "{out}");
    }

    #[test]
    fn peek_renders_the_timeline_and_marks_the_callers_own_posts() {
        let out = render_room(
            "peek",
            "me",
            &json!({
                "room": "wave 23",
                "messages": [
                    { "seq": 1, "sender_label": "lane a (codex on box-b)",
                      "sender_session_id": "other", "body": "started" },
                    { "seq": 2, "sender_label": "me (claude-code on box-a)",
                      "sender_session_id": "me", "body": "on it" },
                    { "seq": 3, "sender_label": "you (human)", "body": "ship it" },
                ],
            }),
        );
        assert!(out.starts_with("wave 23 — 3 message(s):"), "{out}");
        assert!(out.contains("#1 lane a (codex on box-b): started"), "{out}");
        assert!(out.contains("#2 me (claude-code on box-a) (you): on it"), "{out}");
        assert!(out.contains("#3 you (human): ship it"), "{out}");

        let empty = render_room("peek", "me", &json!({ "room": "wave 23", "messages": [] }));
        assert!(empty.contains("no messages yet"), "{empty}");
    }

    #[test]
    fn members_renders_one_line_per_member_with_its_role_and_state() {
        let out = render_room(
            "members",
            "me",
            &json!({ "room": "wave 23", "members": [
                { "session_id": "a", "name": "lane a", "adapter": "codex", "machine": "box-b",
                  "state": "live", "role": "member" },
                { "session_id": "b", "name": "  ", "adapter": "claude-code", "machine": "box-a",
                  "state": "archived", "role": "observer" },
            ] }),
        );
        assert!(out.starts_with("wave 23 — 2 member(s), plus the human:"), "{out}");
        assert!(out.contains("- a [member] lane a · codex on box-b · live"), "{out}");
        assert!(
            out.contains("- b [observer] (unnamed) · claude-code on box-a · archived"),
            "{out}"
        );
    }

    #[test]
    fn remote_peers_render_as_another_cctui_not_an_unknown_adapter() {
        let out = render_peers(&json!({ "peers": [
            { "session_id": "remote:5f0c", "name": "bob's agent", "adapter": null,
              "machine": "b.example", "state": "live", "relation": "remote" },
        ] }));
        assert!(
            out.contains(
                "- remote:5f0c [remote] bob's agent · on another cctui (b.example) · live"
            ),
            "{out}"
        );
    }

    #[test]
    fn a_remote_send_says_whether_it_was_delivered_queued_or_held() {
        let sent = render_sent("remote:x", "remote", "delivered");
        assert!(sent.starts_with("delivered to remote:x"), "{sent}");
        assert!(render_sent("remote:x", "remote", "queued").starts_with("queued for remote:x"));
        let held = render_sent("remote:x", "remote", "awaiting_review");
        assert!(held.contains("reviews outbound"), "{held}");
        assert!(render_sent("s1", "sibling", "delivered").contains("CctuiSend back"));
    }

    #[test]
    fn a_remote_room_renders_its_post_status_and_label_only_members() {
        let post = render_room(
            "post",
            "me",
            &json!({
                "room": "wave 23", "room_id": "remote:x", "remote": true, "status": "queued",
            }),
        );
        assert!(post.contains("another cctui (queued)"), "{post}");
        assert!(!post.contains("#0"), "{post}");

        let members = render_room(
            "members",
            "me",
            &json!({
                "room": "wave 23", "members": ["alice (claude-code on box-a)", "bob (remote)"],
            }),
        );
        assert!(members.contains("- alice (claude-code on box-a)"), "{members}");
        assert!(members.contains("- bob (remote)"), "{members}");

        let local = render_room(
            "members",
            "me",
            &json!({ "room": "wave 23", "members": [
                { "session_id": "remote:y", "name": "carol", "adapter": null,
                  "machine": "c.example", "state": "live", "remote": true },
            ] }),
        );
        assert!(
            local.contains("- remote:y [remote] carol · on another cctui (c.example)"),
            "{local}"
        );
    }

    #[test]
    fn tool_is_unavailable_without_a_machine_key() {
        assert!(!is_available(""));
        assert!(!is_available("   "));
        assert!(is_available("machine-key"));
    }
}
