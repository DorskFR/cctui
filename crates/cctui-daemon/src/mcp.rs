//! `cctui-daemon mcp-agent` — the stdio MCP server a claude session is launched
//! with, exposing the `CctuiAgent` and `CctuiUsage` tools.
//!
//! The subcommand is a thin relay, mirroring `ask-hook`: it speaks MCP on
//! stdio and forwards each `tools/call` to the long-lived daemon over its local
//! Unix socket, which owns the machine key and the spawn path. The session id is
//! fixed by the `--session` argv the daemon wrote into the session's MCP config,
//! so a session can never ask on another session's behalf.
//!
//! Calls run concurrently — one thread per `tools/call`, replies keyed by
//! JSON-RPC id — so parallel child spawns actually run in parallel. While a
//! call waits, the daemon's interim progress frames are forwarded as MCP
//! `notifications/progress` (when the client sent a `progressToken`), which
//! resets the client's tool idle timeout: a long-running child no longer
//! looks like a dead call.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

pub const TOOL_NAME: &str = "CctuiAgent";
pub const USAGE_TOOL_NAME: &str = "CctuiUsage";
pub const PEERS_TOOL_NAME: &str = "CctuiPeers";
pub const SEND_TOOL_NAME: &str = "CctuiSend";
pub const HISTORY_TOOL_NAME: &str = "CctuiHistory";
pub const ROOM_TOOL_NAME: &str = "CctuiRoom";
pub const USER_ACTION_ADD_TOOL_NAME: &str = "CctuiUserActionAdd";
pub const USER_ACTION_TICK_TOOL_NAME: &str = "CctuiUserActionTick";

/// A limits lookup is one cached server read; it must never hold a turn open
/// the way a followed child does. The peer tools are the same shape: one
/// server round-trip, no child to wait for.
const USAGE_TIMEOUT: Duration = Duration::from_secs(30);

/// Socket `kind` of every tool that is a single server round-trip.
const ROUND_TRIP_KINDS: &[(&str, &str)] = &[
    (USAGE_TOOL_NAME, "usage"),
    (PEERS_TOOL_NAME, "peers"),
    (SEND_TOOL_NAME, "send_peer"),
    (HISTORY_TOOL_NAME, "peer_history"),
    (ROOM_TOOL_NAME, "room"),
    (USER_ACTION_ADD_TOOL_NAME, "user_action_add"),
    (USER_ACTION_TICK_TOOL_NAME, "user_action_tick"),
];

/// The socket `kind` a tool call becomes. `spawn_agent` is the only kind that
/// follows a child; every other takes [`USAGE_TIMEOUT`].
#[must_use]
pub fn tool_kind(name: &str) -> Option<&'static str> {
    if name == TOOL_NAME {
        return Some("spawn_agent");
    }
    ROUND_TRIP_KINDS.iter().find(|(tool, _)| *tool == name).map(|(_, kind)| *kind)
}

/// The tool a socket `kind` came from, for naming it in an error.
fn tool_of_kind(kind: &str) -> &'static str {
    ROUND_TRIP_KINDS.iter().find(|(_, k)| *k == kind).map_or(TOOL_NAME, |(tool, _)| *tool)
}

/// MCP protocol revision this server implements.
const PROTOCOL_VERSION: &str = "2024-11-05";

/// Socket line protocol revision: ≥2 tells the daemon this relay understands
/// interim `progress` frames before the final result line.
const SOCKET_PROTO: u64 = 2;

/// Ceiling on a single tool call, and the default when the call names none.
/// Generous: a child review session can legitimately run for many minutes.
const DEFAULT_TIMEOUT_SECS: u64 = 1800;
const MAX_TIMEOUT_SECS: u64 = 7200;

/// The `CctuiAgent` input schema, as advertised to the model.
#[must_use]
pub fn tool_schema() -> Value {
    json!({
        "name": TOOL_NAME,
        "description": "Spawn a cctui subagent session, follow it while it works, and return \
    its final message. The child is a real cctui session: it appears nested under this one in \
    the UI, its token usage is metered, its spend is capped, and it can be killed. Progress \
    (current tool, status, latest message) streams back while it runs. Parallel calls are \
    supported. To send a follow-up prompt to a child from an earlier call, pass its session_id \
    (returned in the reply) together with the new prompt.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "adapter": {
                    "type": "string",
                    "description": "Harness to run the child under, e.g. \"opencode\", \
    \"codex\", \"claude_code\". Only the adapters this session is permitted to spawn are \
    accepted. Ignored when session_id is set.",
                },
                "prompt": {
                    "type": "string",
                    "description": "The task for the child agent (or the follow-up message \
    when session_id is set).",
                },
                "session_id": {
                    "type": "string",
                    "description": "Session id of a child spawned earlier by this session: \
    send `prompt` to it as a follow-up and wait for its answer instead of spawning anew.",
                },
                "model": {
                    "type": "string",
                    "description": "REQUIRED. Model id to run the child on — there is no \
    account default, and a call without one is rejected. Known claude_code ids: \
    \"claude-opus-5[1m]\", \"claude-opus-5\", \"claude-sonnet-5\", \"claude-haiku-4-5\", \
    \"claude-fable-5\"; codex: \"gpt-5.6-sol\", \"gpt-5.6-terra\". An alias from the \
    account's own catalog also works. Ignored when session_id is set, but still name the \
    child's model so the call records what it is talking to.",
                },
                "agent_profile": {
                    "type": "string",
                    "description": "Named agent profile to run under, e.g. \"cctui-reviewer\" \
    (a locked-down opencode reviewer).",
                },
                "permission_mode": {
                    "type": "string",
                    "enum": ["yolo", "auto", "ask"],
                    "description": "Child permission posture, never more permissive than this \
    session's own (ask < auto < yolo). Default: this session's posture.",
                },
                "name": {
                    "type": "string",
                    "description": "Display name for the child session in the UI.",
                },
                "budget_usd": {
                    "type": "number",
                    "description": "Dollar ceiling for this child's own spend. Must not exceed \
    this session's permitted maximum; omit to inherit it.",
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory for the child. Defaults to this session's.",
                },
                "timeout_secs": {
                    "type": "integer",
                    "description": "How long to wait for the child before giving up \
    (default 1800, max 7200). Expiry is not a failure of the child: it keeps running, its \
    work stays on disk, and it can be reattached via session_id. Raise it for work that \
    routinely runs long. The effective window is echoed in the result.",
                },
            },
            "required": ["prompt", "model"],
            "additionalProperties": false,
        },
    })
}

/// The `CctuiUsage` input schema. No required arguments: the session id is
/// already baked into this relay's argv, so the tool always answers for the
/// caller and can never be pointed at another session.
#[must_use]
pub fn usage_tool_schema() -> Value {
    json!({
        "name": USAGE_TOOL_NAME,
        "description": "Report the rate limits and budget that apply to THIS session: the \
    account it is pinned to (which may be a shared or pool-elected one, not your own), that \
    account's usage windows, the caps in force, this session's dollar spend, and whether each \
    model it could run on is currently allowed or soft-limit blocked. Use it before dispatching \
    a batch of work, and when deciding which model to give a child: a blocked model wastes the \
    whole fan-out on 429s. Returns a one-line summary followed by the full JSON.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "model": {
                    "type": "string",
                    "description": "Ask about one model instead of this session's current one. \
    The per-model map is returned either way.",
                },
            },
            "required": [],
            "additionalProperties": false,
        },
    })
}

/// `CctuiPeers`: no arguments — the roster is whatever THIS session may address,
/// and the session id is already baked into the relay's argv.
#[must_use]
pub fn peers_tool_schema() -> Value {
    json!({
        "name": PEERS_TOOL_NAME,
        "description": "List the cctui sessions this session is allowed to talk to: its parent, \
    its children, its siblings, and any session explicitly shared with it — across machines and \
    across harnesses (claude_code, codex, opencode). Each entry gives session_id, name, adapter, \
    machine, state (live / ended / archived) and the relation. Use it before CctuiSend or \
    CctuiHistory: an id not on this list is refused.",
        "inputSchema": {
            "type": "object",
            "properties": {},
            "required": [],
            "additionalProperties": false,
        },
    })
}

/// `CctuiSend`: one message into a peer's turn queue.
#[must_use]
pub fn send_tool_schema() -> Value {
    json!({
        "name": SEND_TOOL_NAME,
        "description": "Send a message to another cctui session, on any machine and under any \
    harness. It arrives as a turn in that session, labelled as coming from this one, and appears \
    in both transcripts as a peer message. This is not a request/response call: the peer is a \
    session with its own work, not a subagent — it answers when and if it chooses, by calling \
    CctuiSend back at you. Only sessions CctuiPeers lists may be addressed; an ended or archived \
    peer cannot receive anything (read it with CctuiHistory instead). Rate-limited to 10 messages \
    a minute, and capped in size — send a pointer (a path, a session id), not a payload.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "The peer to send to, as reported by CctuiPeers.",
                },
                "message": {
                    "type": "string",
                    "description": "What to say. Write it for another agent: state what you need \
    and what you already know, not a greeting.",
                },
            },
            "required": ["session_id", "message"],
            "additionalProperties": false,
        },
    })
}

/// `CctuiHistory`: a bounded page of a peer's transcript. Reads Postgres, so it
/// answers for an archived session and for one whose machine is long gone.
#[must_use]
pub fn history_tool_schema() -> Value {
    json!({
        "name": HISTORY_TOOL_NAME,
        "description": "Read the conversation of another cctui session — what it was asked and \
    what it did. Works for a live session, an ended one, and an archived one whose machine no \
    longer exists: cctui keeps the transcript. Same addressing rules as CctuiSend (see \
    CctuiPeers). Returns compact markdown by default, newest events first-priority within a size \
    budget; page backwards with `before` using the oldest seq the previous call reported. The \
    human running the target session sees that you consulted it.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "session_id": {
                    "type": "string",
                    "description": "The peer whose history to read, as reported by CctuiPeers.",
                },
                "before": {
                    "type": "integer",
                    "description": "Return events older than this seq — the `oldest seq` of the \
    previous page. This is how you page back through a long conversation.",
                },
                "after": {
                    "type": "integer",
                    "description": "Return events newer than this seq, oldest-first: a delta \
    catch-up on a session you already read.",
                },
                "limit": {
                    "type": "integer",
                    "description": "Events to read (default 200, max 1000). The reply is also \
    bounded by a byte budget, so a big transcript comes back truncated with a cursor.",
                },
                "roles": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Keep only these roles, e.g. [\"user\", \"assistant\"] for the \
    conversation without tool noise. Known roles: user, assistant, peer, thinking, tool, result, \
    mcp, error, summary, compact, system. Omit for everything.",
                },
                "format": {
                    "type": "string",
                    "enum": ["markdown", "json"],
                    "description": "`markdown` (default) is what you want to read; `json` returns \
    the raw normalized events.",
                },
            },
            "required": ["session_id"],
            "additionalProperties": false,
        },
    })
}

/// `CctuiRoom`: post to, read, or inspect a room this session belongs to.
#[must_use]
pub fn room_tool_schema() -> Value {
    json!({
        "name": ROOM_TOOL_NAME,
        "description": "Broadcast to your cctui ROOM: the named group of sessions a human put \
    you in. You cannot join or leave one yourself. `post` delivers your message to every other \
    LIVE session in the room as a turn, and reports per-session whether it landed — a session \
    that is archived, ended or whose machine is offline is skipped and named, not queued, so \
    re-post later if it matters. IMPORTANT: only an explicit post reaches the room — your ordinary \
    replies stay in your own conversation, so working normally never echoes you into the room. \
    Messages you receive wrapped in <cctui-room> came from another session in the room, not from \
    the human who runs you. `peek` reads the room's past messages, `members` lists who is in it. \
    Being in a room also lets you use CctuiPeers, CctuiSend and CctuiHistory on its sessions. Omit \
    room_id: you are in at most one room. A post counts against the same rate limit as CctuiSend \
    and is size-capped — post a pointer, not a payload.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["post", "peek", "members"],
                    "description": "`post` to say something to the room, `peek` to read the \
    timeline, `members` to see who is in it.",
                },
                "room_id": {
                    "type": "string",
                    "description": "Rarely needed: a session is in at most one room, which is \
    used when this is omitted.",
                },
                "message": {
                    "type": "string",
                    "description": "What to post. Required for `post`, ignored otherwise. Write \
    it for the other agents and the human reading along.",
                },
            },
            "required": ["action"],
            "additionalProperties": false,
        },
    })
}

/// The split both user-action tools must state, so a model does not file its own
/// steps here or the user's here into its own plan.
const USER_ACTION_SPLIT: &str = "This list is what YOU are waiting on from the USER — approvals, \
    commands only they can run, secrets, decisions, a yubikey touch. Track your OWN work with \
    TodoWrite / update_plan instead; never put your own steps here, and never put a request for \
    the user into your task list. The tools never block: they return immediately and the user \
    answers in their own time.";

/// The `CctuiUserActionAdd` input schema.
#[must_use]
pub fn user_action_add_schema() -> Value {
    json!({
        "name": USER_ACTION_ADD_TOOL_NAME,
        "description": format!(
            "Pin one thing you need from the user onto this session's \"needs you\" list, shown as \
    a card in the conversation and a badge on the session card, so the request does not scroll away. \
    {USER_ACTION_SPLIT} Returns the item's id and the whole current list, including items the user \
    ticked in the UI — read it to learn what they have already done. Re-adding an identical open \
    title returns the existing item instead of a duplicate."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "One imperative line addressed to the user, e.g. \"Approve PR \
    #12\" or \"Run `! gcloud auth login`\". Trimmed to 120 characters; an empty title is rejected.",
                },
                "detail": {
                    "type": "string",
                    "description": "Markdown: why it is needed, the exact command to run, links.",
                },
                "kind": {
                    "type": "string",
                    "enum": ["action", "input", "decision"],
                    "description": "action = the user must DO something; input = the user must TELL \
    you something; decision = the user must CHOOSE. Default: action.",
                },
                "blocking": {
                    "type": "boolean",
                    "description": "True when you cannot continue your main line of work without \
    it. A blocking item notifies the user the way a permission prompt does, so reserve it for a \
    real stop.",
                },
            },
            "required": ["title"],
            "additionalProperties": false,
        },
    })
}

/// The `CctuiUserActionTick` input schema.
#[must_use]
pub fn user_action_tick_schema() -> Value {
    json!({
        "name": USER_ACTION_TICK_TOOL_NAME,
        "description": format!(
            "Resolve one item on this session's \"needs you\" list by id — the user did it, or it \
    is no longer needed. {USER_ACTION_SPLIT} Returns the whole current list. An unknown id comes \
    back as an error that still carries the list, so you can re-read the real ids."
        ),
        "inputSchema": {
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Id of the item, as returned by CctuiUserActionAdd or a previous \
    tick.",
                },
                "status": {
                    "type": "string",
                    "enum": ["done", "dropped"],
                    "description": "done = you saw it happen (the user said so, or the command's \
    output landed); dropped = no longer needed.",
                },
                "note": {
                    "type": "string",
                    "description": "Short outcome, e.g. \"token received\".",
                },
            },
            "required": ["id", "status"],
            "additionalProperties": false,
        },
    })
}

/// Every tool this relay advertises, in a stable order.
///
/// One list, one relay: registering it for codex or opencode
/// (`adapters::agent_mcp`) offers exactly the same surface as `claude_code`'s
/// `--mcp-config`.
#[must_use]
pub fn tool_schemas() -> Vec<Value> {
    vec![
        tool_schema(),
        usage_tool_schema(),
        peers_tool_schema(),
        send_tool_schema(),
        history_tool_schema(),
        room_tool_schema(),
        user_action_add_schema(),
        user_action_tick_schema(),
    ]
}

/// Clamp a caller-supplied timeout into the supported range.
#[must_use]
pub fn resolve_timeout(requested: Option<u64>) -> Duration {
    Duration::from_secs(requested.unwrap_or(DEFAULT_TIMEOUT_SECS).clamp(1, MAX_TIMEOUT_SECS))
}

/// Build the `.mcp.json`-shaped config registering this server for a session.
///
/// `exe` is the daemon binary; the session id and socket are baked into argv so
/// the tool call carries no session identity of its own.
#[must_use]
pub fn mcp_config(exe: &str, session_id: &str, sock: &Path) -> Value {
    json!({
        "mcpServers": {
            "cctui": {
                "type": "stdio",
                "command": exe,
                "args": [
                    "mcp-agent",
                    "--session", session_id,
                    "--sock", sock.to_string_lossy(),
                ],
            }
        }
    })
}

/// Serialize one JSON-RPC frame to stdout. The lock is per-line so concurrent
/// tool calls interleave whole frames, never bytes.
#[derive(Clone)]
struct Outbox(Arc<Mutex<std::io::Stdout>>);

impl Outbox {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(std::io::stdout())))
    }

    fn send(&self, frame: &Value) {
        if let Ok(mut out) = self.0.lock() {
            let _ = writeln!(out, "{frame}");
            let _ = out.flush();
        }
    }
}

fn reply(id: Option<&Value>, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id.cloned().unwrap_or(Value::Null), "result": result })
}

/// A tool result carrying `text`. `is_error` marks a failed call so the model
/// sees the failure instead of a hang.
fn tool_result(id: Option<&Value>, text: &str, is_error: bool) -> Value {
    reply(id, &json!({ "content": [{ "type": "text", "text": text }], "isError": is_error }))
}

fn progress_notification(token: &Value, seq: u64, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": { "progressToken": token, "progress": seq, "message": message },
    })
}

/// The `_meta.progressToken` of a request, when the client sent one.
fn progress_token(req: &Value) -> Option<Value> {
    req.pointer("/params/_meta/progressToken").cloned().filter(|t| !t.is_null())
}

/// Handle one decoded JSON-RPC request inline; `None` for notifications (no
/// reply) AND for `tools/call`, which replies asynchronously from its own
/// thread via the outbox.
fn handle_request(session_id: &str, sock: &Path, req: &Value, outbox: &Outbox) -> Option<Value> {
    let method = req.get("method").and_then(Value::as_str)?;
    let id = req.get("id");
    match method {
        "initialize" => {
            announce_ready(session_id, sock);
            Some(reply(
                id,
                &json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "cctui", "version": env!("CARGO_PKG_VERSION") },
                }),
            ))
        }
        "tools/list" => Some(reply(id, &json!({ "tools": tool_schemas() }))),
        "tools/call" => {
            let params = req.get("params");
            let name = params.and_then(|p| p.get("name")).and_then(Value::as_str).unwrap_or("");
            let Some(kind) = tool_kind(name) else {
                return Some(tool_result(id, &format!("unknown tool {name:?}"), true));
            };
            let args =
                params.and_then(|p| p.get("arguments")).cloned().unwrap_or_else(|| json!({}));
            let id = id.cloned();
            let token = progress_token(req);
            let session_id = session_id.to_owned();
            let sock = sock.to_owned();
            let outbox = outbox.clone();
            std::thread::spawn(move || {
                let (text, is_error) =
                    call_daemon(&session_id, &sock, kind, &args, token.as_ref(), &outbox);
                outbox.send(&tool_result(id.as_ref(), &text, is_error));
            });
            None
        }
        _ if id.is_none() => None,
        _ => Some(reply(id, &json!({}))),
    }
}

/// Send the tool call to the daemon, forwarding interim progress frames, and
/// block on the final result line. Every failure returns text: the model must
/// see an error, never a hang.
fn call_daemon(
    session_id: &str,
    sock: &Path,
    kind: &str,
    args: &Value,
    token: Option<&Value>,
    outbox: &Outbox,
) -> (String, bool) {
    let tool = tool_of_kind(kind);
    let timeout = if kind == "spawn_agent" {
        resolve_timeout(args.get("timeout_secs").and_then(Value::as_u64))
    } else {
        USAGE_TIMEOUT
    };
    let request = json!({
        "kind": kind,
        "session_id": session_id,
        "args": args,
        "timeout_secs": timeout.as_secs(),
        "proto": SOCKET_PROTO,
    });
    let stream = match UnixStream::connect(sock) {
        Ok(s) => s,
        Err(err) => {
            return (format!("{tool} unavailable: cannot reach the cctui daemon ({err})"), true);
        }
    };
    // Outlive the daemon's own wait so the daemon's timeout message wins.
    let _ = stream.set_read_timeout(Some(timeout + Duration::from_secs(30)));
    let mut writer = &stream;
    if writeln!(writer, "{request}").and_then(|()| writer.flush()).is_err() {
        return (format!("{tool} failed: could not send the request to the daemon"), true);
    }
    let mut reader = BufReader::new(&stream);
    let mut seq: u64 = 0;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
            return (
                format!("{tool} failed: the daemon closed the connection without a result"),
                true,
            );
        }
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            return (format!("{tool} failed: malformed daemon reply"), true);
        };
        if let Some(progress) = frame.get("progress").and_then(Value::as_str) {
            if let Some(token) = token {
                seq += 1;
                outbox.send(&progress_notification(token, seq, progress));
            }
            continue;
        }
        let ok = frame.get("ok").and_then(Value::as_bool).unwrap_or(false);
        let text = frame
            .get(if ok { "result" } else { "error" })
            .and_then(Value::as_str)
            .unwrap_or("no output")
            .to_owned();
        return (text, !ok);
    }
}

/// One fire-and-forget line on the daemon socket, ignoring any reply. Used for
/// signalling that must never delay or fail the caller.
fn notify_daemon(sock: &Path, frame: &Value) {
    let Ok(stream) = UnixStream::connect(sock) else { return };
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut writer = &stream;
    let _ = writeln!(writer, "{frame}").and_then(|()| writer.flush());
}

/// Tell the daemon this session's relay is up, so the `SessionStart` hook
/// holding the first turn can release it. Off-thread: `initialize` must be
/// answered immediately even if the daemon socket is slow or absent.
fn announce_ready(session_id: &str, sock: &Path) {
    let frame = json!({ "kind": "relay_ready", "session_id": session_id, "proto": SOCKET_PROTO });
    let sock = sock.to_owned();
    std::thread::spawn(move || notify_daemon(&sock, &frame));
}

/// Block until this session's relay has announced itself, or `timeout` elapses.
///
/// Infallible on purpose: the hook that calls this releases the first turn
/// either way, so a daemon that is unreachable costs the launch nothing.
pub fn wait_ready(session_id: &str, sock: &Path, timeout: Duration) {
    let request = json!({
        "kind": "relay_wait",
        "session_id": session_id,
        "timeout_secs": timeout.as_secs().max(1),
        "proto": SOCKET_PROTO,
    });
    let Ok(stream) = UnixStream::connect(sock) else { return };
    let _ = stream.set_read_timeout(Some(timeout + Duration::from_secs(5)));
    let mut writer = &stream;
    if writeln!(writer, "{request}").and_then(|()| writer.flush()).is_err() {
        return;
    }
    let mut line = String::new();
    let _ = BufReader::new(&stream).read_line(&mut line);
}

/// Serve MCP on stdio until the client closes it.
pub fn run(session_id: &str, sock: &Path) -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let outbox = Outbox::new();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if let Some(resp) = handle_request(session_id, sock, &req, &outbox) {
            outbox.send(&resp);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn handle(session_id: &str, sock: &Path, req: &Value) -> Option<Value> {
        handle_request(session_id, sock, req, &Outbox::new())
    }

    #[test]
    fn tool_schema_names_the_tool_and_its_required_args() {
        let schema = tool_schema();
        assert_eq!(schema["name"], TOOL_NAME);
        assert_eq!(schema["inputSchema"]["required"], json!(["prompt", "model"]));
        let model_doc =
            schema["inputSchema"]["properties"]["model"]["description"].as_str().unwrap();
        assert!(model_doc.contains("REQUIRED"), "{model_doc}");
        assert!(model_doc.contains("claude-opus-5[1m]"), "{model_doc}");
        assert!(model_doc.contains("no account default"), "{model_doc}");
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        for key in [
            "adapter",
            "prompt",
            "session_id",
            "model",
            "agent_profile",
            "permission_mode",
            "name",
            "budget_usd",
            "cwd",
            "timeout_secs",
        ] {
            assert!(props.contains_key(key), "{key} missing from the schema");
        }
        assert_eq!(props["budget_usd"]["type"], "number");
        assert_eq!(schema["inputSchema"]["additionalProperties"], json!(false));
    }

    #[test]
    fn tool_schema_round_trips_as_json() {
        let raw = serde_json::to_string(&tool_schema()).unwrap();
        let back: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, tool_schema());
    }

    #[test]
    fn initialize_advertises_tools() {
        let req = json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize" });
        let resp = handle("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        assert_eq!(resp["id"], json!(1));
        assert_eq!(resp["result"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_returns_the_agent_usage_peer_and_user_action_tools() {
        let req = json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" });
        let resp = handle("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                TOOL_NAME,
                USAGE_TOOL_NAME,
                PEERS_TOOL_NAME,
                SEND_TOOL_NAME,
                HISTORY_TOOL_NAME,
                ROOM_TOOL_NAME,
                USER_ACTION_ADD_TOOL_NAME,
                USER_ACTION_TICK_TOOL_NAME,
            ]
        );
    }

    /// A room tool that let a model pick its own action strings would fail
    /// server-side; the enum is what keeps the three actions honest.
    #[test]
    fn the_room_tool_enumerates_its_actions_and_states_the_loop_guard() {
        let schema = room_tool_schema();
        assert_eq!(schema["inputSchema"]["required"], json!(["action"]));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        assert_eq!(props["action"]["enum"], json!(["post", "peek", "members"]));
        for key in ["action", "room_id", "message"] {
            assert!(props.contains_key(key), "{key} missing from the schema");
        }
        let desc = schema["description"].as_str().unwrap();
        assert!(
            desc.contains("only an explicit post reaches the room"),
            "the loop guard must be in the tool description too: {desc}"
        );
        assert!(desc.contains("<cctui-room>"), "{desc}");
        assert!(desc.contains("at most one room"), "{desc}");
        assert!(
            desc.contains("skipped and named, not queued"),
            "a broadcast is best effort; the model must not assume delivery: {desc}"
        );
        assert_eq!(tool_kind(ROOM_TOOL_NAME), Some("room"));
    }

    /// Every advertised tool must map onto a socket kind, or a model would call
    /// something the daemon answers "unsupported request kind" to.
    #[test]
    fn every_advertised_tool_has_a_socket_kind_and_a_closed_schema() {
        for schema in tool_schemas() {
            let name = schema["name"].as_str().unwrap();
            let kind = tool_kind(name).unwrap_or_else(|| panic!("{name} has no socket kind"));
            assert!(!kind.is_empty());
            assert_eq!(schema["inputSchema"]["additionalProperties"], json!(false), "{name}");
            assert!(schema["inputSchema"]["required"].is_array(), "{name}");
            assert!(
                schema["description"].as_str().is_some_and(|d| d.len() > 60),
                "{name} needs a description a model can act on"
            );
        }
        assert!(tool_kind("NoSuchTool").is_none());
    }

    #[test]
    fn the_peer_tool_kinds_are_the_ones_the_daemon_parses() {
        assert_eq!(tool_kind(TOOL_NAME), Some("spawn_agent"));
        assert_eq!(tool_kind(USAGE_TOOL_NAME), Some("usage"));
        assert_eq!(tool_kind(PEERS_TOOL_NAME), Some("peers"));
        assert_eq!(tool_kind(SEND_TOOL_NAME), Some("send_peer"));
        assert_eq!(tool_kind(HISTORY_TOOL_NAME), Some("peer_history"));
    }

    #[test]
    fn the_roster_tool_takes_no_arguments_at_all() {
        let schema = peers_tool_schema();
        assert_eq!(schema["inputSchema"]["required"], json!([]));
        assert!(schema["inputSchema"]["properties"].as_object().unwrap().is_empty());
        let desc = schema["description"].as_str().unwrap();
        for word in ["parent", "children", "siblings", "shared", "archived"] {
            assert!(desc.contains(word), "{word} missing: {desc}");
        }
    }

    /// The send tool must say it is not request/response: a model that treats a
    /// peer like a subagent blocks waiting for an answer that never comes.
    #[test]
    fn the_send_tool_requires_a_target_and_a_message_and_warns_it_is_one_way() {
        let schema = send_tool_schema();
        assert_eq!(schema["inputSchema"]["required"], json!(["session_id", "message"]));
        let desc = schema["description"].as_str().unwrap();
        assert!(desc.contains("not a request/response"), "{desc}");
        assert!(desc.contains("CctuiPeers"), "{desc}");
        assert!(desc.contains("archived"), "{desc}");
    }

    #[test]
    fn the_history_tool_documents_its_cursors_roles_and_formats() {
        let schema = history_tool_schema();
        assert_eq!(schema["inputSchema"]["required"], json!(["session_id"]));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        for key in ["session_id", "before", "after", "limit", "roles", "format"] {
            assert!(props.contains_key(key), "{key} missing from the schema");
        }
        assert_eq!(props["roles"]["type"], "array");
        assert_eq!(props["format"]["enum"], json!(["markdown", "json"]));
        let desc = schema["description"].as_str().unwrap();
        assert!(desc.contains("archived"), "{desc}");
        assert!(desc.contains("no longer exists"), "{desc}");
    }

    #[test]
    fn a_peer_call_reaches_the_daemon_under_its_own_kind() {
        for (tool, kind, args) in [
            (PEERS_TOOL_NAME, "peers", json!({})),
            (SEND_TOOL_NAME, "send_peer", json!({ "session_id": "t", "message": "hi" })),
            (HISTORY_TOOL_NAME, "peer_history", json!({ "session_id": "t", "limit": 10 })),
            (ROOM_TOOL_NAME, "room", json!({ "action": "post", "message": "hi" })),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let sock_path = dir.path().join("agent.sock");
            let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
            let expect_kind = kind.to_owned();
            let server = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                assert_eq!(req["kind"], json!(expect_kind));
                assert_eq!(req["session_id"], json!("s1"));
                assert_eq!(
                    req["timeout_secs"],
                    json!(USAGE_TIMEOUT.as_secs()),
                    "a peer call is a round-trip, not a child follow",
                );
                writeln!(stream, "{}", json!({ "ok": true, "result": "fine" })).unwrap();
            });
            let (text, is_error) = call_daemon("s1", &sock_path, kind, &args, None, &Outbox::new());
            server.join().unwrap();
            assert!(!is_error, "{tool}: {text}");
            assert_eq!(text, "fine");
        }
    }

    #[test]
    fn a_dead_socket_names_the_peer_tool_that_failed() {
        for (tool, kind) in [
            (PEERS_TOOL_NAME, "peers"),
            (SEND_TOOL_NAME, "send_peer"),
            (HISTORY_TOOL_NAME, "peer_history"),
            (ROOM_TOOL_NAME, "room"),
        ] {
            let (text, is_error) = call_daemon(
                "s1",
                Path::new("/nonexistent/cctui-agent.sock"),
                kind,
                &json!({}),
                None,
                &Outbox::new(),
            );
            assert!(is_error);
            assert!(text.starts_with(tool), "{text}");
        }
    }

    #[test]
    fn the_usage_tool_needs_no_arguments_and_takes_only_an_optional_model() {
        let schema = usage_tool_schema();
        assert_eq!(schema["name"], USAGE_TOOL_NAME);
        assert_eq!(schema["inputSchema"]["required"], json!([]));
        assert_eq!(schema["inputSchema"]["additionalProperties"], json!(false));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        assert_eq!(props.keys().collect::<Vec<_>>(), vec!["model"]);
        let desc = schema["description"].as_str().unwrap();
        assert!(desc.contains("THIS session"), "{desc}");
        assert!(desc.contains("blocked"), "{desc}");
    }

    #[test]
    fn a_usage_call_reaches_the_daemon_as_a_usage_kind_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["kind"], json!("usage"));
            assert_eq!(req["session_id"], json!("s1"));
            assert_eq!(req["args"]["model"], json!("claude-opus-5"));
            writeln!(stream, "{}", json!({ "ok": true, "result": "5h 46% · weekly 71%" })).unwrap();
        });
        let (text, is_error) = call_daemon(
            "s1",
            &sock_path,
            "usage",
            &json!({ "model": "claude-opus-5" }),
            None,
            &Outbox::new(),
        );
        server.join().unwrap();
        assert!(!is_error);
        assert_eq!(text, "5h 46% · weekly 71%");
    }

    #[test]
    fn a_dead_socket_fails_the_usage_call_by_name_instead_of_hanging() {
        let (text, is_error) = call_daemon(
            "s1",
            Path::new("/nonexistent/cctui-agent.sock"),
            "usage",
            &json!({}),
            None,
            &Outbox::new(),
        );
        assert!(is_error);
        assert!(text.starts_with(USAGE_TOOL_NAME), "{text}");
        assert!(text.contains("cannot reach the cctui daemon"), "{text}");
    }

    #[test]
    fn the_add_schema_requires_only_a_title_and_states_the_todo_split() {
        let schema = user_action_add_schema();
        assert_eq!(schema["name"], USER_ACTION_ADD_TOOL_NAME);
        assert_eq!(schema["inputSchema"]["required"], json!(["title"]));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        for key in ["title", "detail", "kind", "blocking"] {
            assert!(props.contains_key(key), "{key} missing from the schema");
        }
        assert_eq!(props["kind"]["enum"], json!(["action", "input", "decision"]));
        assert_eq!(props["blocking"]["type"], "boolean");
        let desc = schema["description"].as_str().unwrap();
        assert!(desc.contains("TodoWrite"), "{desc}");
        assert!(desc.contains("waiting on from the USER"), "{desc}");
        assert_eq!(tool_kind(USER_ACTION_ADD_TOOL_NAME), Some("user_action_add"));
    }

    #[test]
    fn the_tick_schema_requires_an_id_and_a_terminal_status() {
        let schema = user_action_tick_schema();
        assert_eq!(schema["name"], USER_ACTION_TICK_TOOL_NAME);
        assert_eq!(schema["inputSchema"]["required"], json!(["id", "status"]));
        let props = schema["inputSchema"]["properties"].as_object().unwrap();
        assert_eq!(props["status"]["enum"], json!(["done", "dropped"]));
        assert!(props.contains_key("note"));
        let desc = schema["description"].as_str().unwrap();
        assert!(desc.contains("TodoWrite"), "{desc}");
        assert_eq!(tool_kind(USER_ACTION_TICK_TOOL_NAME), Some("user_action_tick"));
    }

    #[test]
    fn user_action_schemas_round_trip_as_json() {
        for schema in [user_action_add_schema(), user_action_tick_schema()] {
            let raw = serde_json::to_string(&schema).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&raw).unwrap(), schema);
        }
    }

    #[test]
    fn an_add_call_reaches_the_daemon_as_a_user_action_add_kind() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["kind"], json!("user_action_add"));
            assert_eq!(req["session_id"], json!("s1"));
            assert_eq!(req["args"]["title"], json!("Approve PR #12"));
            assert_eq!(req["timeout_secs"], json!(USAGE_TIMEOUT.as_secs()));
            writeln!(stream, "{}", json!({ "ok": true, "result": "user actions (1 open of 1):" }))
                .unwrap();
        });
        let (text, is_error) = call_daemon(
            "s1",
            &sock_path,
            "user_action_add",
            &json!({ "title": "Approve PR #12", "blocking": true }),
            None,
            &Outbox::new(),
        );
        server.join().unwrap();
        assert!(!is_error);
        assert!(text.contains("1 open"), "{text}");
    }

    #[test]
    fn a_dead_socket_fails_a_tick_by_name_instead_of_hanging() {
        let (text, is_error) = call_daemon(
            "s1",
            Path::new("/nonexistent/cctui-agent.sock"),
            "user_action_tick",
            &json!({ "id": "x", "status": "done" }),
            None,
            &Outbox::new(),
        );
        assert!(is_error);
        assert!(text.starts_with(USER_ACTION_TICK_TOOL_NAME), "{text}");
    }

    #[test]
    fn notifications_get_no_reply() {
        let req = json!({ "jsonrpc": "2.0", "method": "notifications/initialized" });
        assert!(handle("s1", Path::new("/tmp/x.sock"), &req).is_none());
    }

    #[test]
    fn unknown_tool_is_an_error_result_not_a_hang() {
        let req = json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": { "name": "SomethingElse", "arguments": {} },
        });
        let resp = handle("s1", Path::new("/tmp/x.sock"), &req).unwrap();
        assert_eq!(resp["result"]["isError"], json!(true));
    }

    #[test]
    fn a_dead_daemon_socket_returns_an_error_result() {
        let (text, is_error) = call_daemon(
            "s1",
            Path::new("/nonexistent/cctui-agent.sock"),
            "spawn_agent",
            &json!({ "adapter": "opencode", "prompt": "hi", "timeout_secs": 1 }),
            None,
            &Outbox::new(),
        );
        assert!(is_error);
        assert!(text.contains("cannot reach the cctui daemon"));
    }

    #[test]
    fn progress_frames_forward_as_notifications_and_final_line_ends_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(req["proto"], json!(SOCKET_PROTO));
            writeln!(stream, "{}", json!({ "progress": "child working · tool: Bash" })).unwrap();
            writeln!(stream, "{}", json!({ "ok": true, "result": "verdict: ship" })).unwrap();
        });
        let (text, is_error) = call_daemon(
            "s1",
            &sock_path,
            "spawn_agent",
            &json!({ "adapter": "codex", "prompt": "go", "timeout_secs": 5 }),
            Some(&json!("tok-1")),
            &Outbox::new(),
        );
        server.join().unwrap();
        assert!(!is_error);
        assert_eq!(text, "verdict: ship");
    }

    #[test]
    fn concurrent_tool_calls_reply_out_of_order() {
        let dir = tempfile::tempdir().unwrap();
        let sock_path = dir.path().join("agent.sock");
        let listener = std::os::unix::net::UnixListener::bind(&sock_path).unwrap();
        let server = std::thread::spawn(move || {
            let mut streams = Vec::new();
            for _ in 0..2 {
                let (stream, _) = listener.accept().unwrap();
                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
                let req: Value = serde_json::from_str(&line).unwrap();
                streams.push((stream, req["args"]["prompt"].as_str().unwrap().to_owned()));
            }
            // Answer in reverse arrival order: the second call must not be
            // blocked behind the first.
            streams.reverse();
            for (mut stream, prompt) in streams {
                writeln!(stream, "{}", json!({ "ok": true, "result": format!("done: {prompt}") }))
                    .unwrap();
            }
        });
        let sock_a = sock_path.clone();
        let a = std::thread::spawn(move || {
            call_daemon(
                "s1",
                &sock_a,
                "spawn_agent",
                &json!({ "adapter": "codex", "prompt": "first", "timeout_secs": 5 }),
                None,
                &Outbox::new(),
            )
        });
        std::thread::sleep(Duration::from_millis(50));
        let b = std::thread::spawn(move || {
            call_daemon(
                "s1",
                &sock_path,
                "spawn_agent",
                &json!({ "adapter": "codex", "prompt": "second", "timeout_secs": 5 }),
                None,
                &Outbox::new(),
            )
        });
        let (text_a, err_a) = a.join().unwrap();
        let (text_b, err_b) = b.join().unwrap();
        server.join().unwrap();
        assert!(!err_a && !err_b);
        assert_eq!(text_a, "done: first");
        assert_eq!(text_b, "done: second");
    }

    #[test]
    fn progress_token_is_read_from_meta() {
        let req = json!({
            "jsonrpc": "2.0", "id": 4, "method": "tools/call",
            "params": {
                "name": TOOL_NAME,
                "_meta": { "progressToken": 7 },
                "arguments": {},
            },
        });
        assert_eq!(progress_token(&req), Some(json!(7)));
        assert!(progress_token(&json!({ "params": {} })).is_none());
    }

    #[test]
    fn timeout_is_clamped_to_the_supported_range() {
        assert_eq!(resolve_timeout(None).as_secs(), DEFAULT_TIMEOUT_SECS);
        assert_eq!(resolve_timeout(Some(60)).as_secs(), 60);
        assert_eq!(resolve_timeout(Some(0)).as_secs(), 1);
        assert_eq!(resolve_timeout(Some(999_999)).as_secs(), MAX_TIMEOUT_SECS);
    }

    #[test]
    fn mcp_config_bakes_the_session_and_socket_into_argv() {
        let cfg = mcp_config("/usr/bin/cctui-daemon", "sess-1", Path::new("/run/cctui/agent.sock"));
        let server = &cfg["mcpServers"]["cctui"];
        assert_eq!(server["command"], "/usr/bin/cctui-daemon");
        assert_eq!(server["type"], "stdio");
        assert_eq!(
            server["args"],
            json!(["mcp-agent", "--session", "sess-1", "--sock", "/run/cctui/agent.sock"])
        );
    }
}
