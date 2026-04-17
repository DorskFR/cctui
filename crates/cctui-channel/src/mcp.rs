//! MCP stdio server. Hand-rolled JSON-RPC 2.0 over line-delimited JSON on
//! stdin/stdout (matches the `StdioServerTransport` wire format).
//!
//! Port of `channel/src/mcp.ts`. Supported surface:
//!   * Requests: `initialize`, `tools/list`, `tools/call` (name=`cctui_reply`)
//!   * Incoming notification: `notifications/claude/channel/permission_request`
//!   * Outgoing notifications:
//!       - `notifications/claude/channel` (TUI → Claude)
//!       - `notifications/claude/channel/permission` (decision)
//!
//! ALL log output MUST go to stderr — stdout is reserved for the protocol.

use std::collections::HashMap;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

const INSTRUCTIONS: &str = concat!(
    "Messages from the TUI operator arrive as <channel source=\"cctui\" sender=\"tui\">. ",
    "These are instructions or questions from the human monitoring your session. ",
    "Read them carefully and act on them. ",
    "Reply using the cctui_reply tool to send a response back to the TUI. ",
    "Always acknowledge TUI messages, even if briefly.",
);

const PROTOCOL_VERSION: &str = "2024-11-05";
const SERVER_NAME: &str = "cctui";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Event emitted to the orchestrator by the reader loop.
#[derive(Debug)]
pub enum McpEvent {
    /// Claude called the `cctui_reply` tool.
    Reply(String),
    /// Claude sent a `notifications/claude/channel/permission_request`.
    PermissionRequest {
        request_id: String,
        tool_name: String,
        description: String,
        input_preview: String,
    },
}

/// Handle for pushing notifications *out* to Claude.
#[derive(Clone)]
pub struct Pusher {
    tx: mpsc::Sender<Value>,
}

impl Pusher {
    pub async fn push_message(&self, content: &str, meta: HashMap<String, String>) {
        let mut meta_json = serde_json::Map::new();
        meta_json.insert("sender".to_string(), Value::String("tui".to_string()));
        for (k, v) in meta {
            meta_json.insert(k, Value::String(v));
        }
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "notifications/claude/channel",
            "params": { "content": content, "meta": meta_json },
        });
        let _ = self.tx.send(msg).await;
    }

    pub async fn send_permission_response(&self, request_id: &str, behavior: &str) {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": "notifications/claude/channel/permission",
            "params": { "request_id": request_id, "behavior": behavior },
        });
        let _ = self.tx.send(msg).await;
    }
}

pub struct McpHandle {
    pub events: mpsc::Receiver<McpEvent>,
    pub pusher: Pusher,
}

/// Spawn the reader + writer tasks. Returns a handle to the orchestrator.
#[must_use]
pub fn serve() -> McpHandle {
    let (out_tx, mut out_rx) = mpsc::channel::<Value>(64);
    let (evt_tx, evt_rx) = mpsc::channel::<McpEvent>(64);

    // Writer task: drain outbound queue → stdout.
    tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(msg) = out_rx.recv().await {
            let mut line = match serde_json::to_vec(&msg) {
                Ok(b) => b,
                Err(err) => {
                    tracing::error!(error = %err, "serialize outbound");
                    continue;
                }
            };
            line.push(b'\n');
            if let Err(err) = stdout.write_all(&line).await {
                tracing::error!(error = %err, "stdout write");
                break;
            }
            if let Err(err) = stdout.flush().await {
                tracing::error!(error = %err, "stdout flush");
                break;
            }
        }
    });

    // Reader task: stdin → parse → respond / event.
    let out_tx_reader = out_tx.clone();
    let evt_tx_reader = evt_tx.clone();
    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    handle_line(&line, &out_tx_reader, &evt_tx_reader).await;
                }
                Ok(None) => {
                    tracing::error!("stdin EOF");
                    break;
                }
                Err(err) => {
                    tracing::error!(error = %err, "stdin read");
                    break;
                }
            }
        }
    });

    McpHandle { events: evt_rx, pusher: Pusher { tx: out_tx } }
}

async fn handle_line(line: &str, out_tx: &mpsc::Sender<Value>, evt_tx: &mpsc::Sender<McpEvent>) {
    let Ok(msg) = serde_json::from_str::<Value>(line) else {
        tracing::error!(line = %line, "invalid JSON");
        return;
    };
    let method = msg.get("method").and_then(Value::as_str);
    let id = msg.get("id").cloned();

    match (method, id) {
        (Some(m), Some(id)) => handle_request(m, id, &msg, out_tx, evt_tx).await,
        (Some(m), None) => handle_notification(m, &msg, evt_tx).await,
        _ => {}
    }
}

async fn handle_request(
    method: &str,
    id: Value,
    msg: &Value,
    out_tx: &mpsc::Sender<Value>,
    evt_tx: &mpsc::Sender<McpEvent>,
) {
    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL_VERSION,
            "serverInfo": { "name": SERVER_NAME, "version": SERVER_VERSION },
            "capabilities": {
                "experimental": {
                    "claude/channel": {},
                    "claude/channel/permission": {},
                },
                "tools": {},
            },
            "instructions": INSTRUCTIONS,
        }),
        "tools/list" => json!({
            "tools": [{
                "name": "cctui_reply",
                "description": "Send a message back to the TUI operator who is monitoring this session",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string", "description": "The message to send to the TUI operator" },
                    },
                    "required": ["text"],
                },
            }],
        }),
        "tools/call" => {
            let name = msg.pointer("/params/name").and_then(Value::as_str).unwrap_or("");
            if name == "cctui_reply" {
                let text = msg
                    .pointer("/params/arguments/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let _ = evt_tx.send(McpEvent::Reply(text)).await;
                json!({ "content": [{ "type": "text", "text": "Message sent to TUI." }] })
            } else {
                let err = json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32601, "message": format!("unknown tool: {name}") },
                });
                let _ = out_tx.send(err).await;
                return;
            }
        }
        "ping" => json!({}),
        _ => {
            let err = json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("method not found: {method}") },
            });
            let _ = out_tx.send(err).await;
            return;
        }
    };
    let resp = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    let _ = out_tx.send(resp).await;
}

async fn handle_notification(method: &str, msg: &Value, evt_tx: &mpsc::Sender<McpEvent>) {
    match method {
        "notifications/initialized" | "notifications/cancelled" => {}
        "notifications/claude/channel/permission_request" => {
            let p = msg.pointer("/params").cloned().unwrap_or(Value::Null);
            let request_id = p.get("request_id").and_then(Value::as_str).unwrap_or("").to_string();
            let tool_name = p.get("tool_name").and_then(Value::as_str).unwrap_or("").to_string();
            let description =
                p.get("description").and_then(Value::as_str).unwrap_or("").to_string();
            let input_preview =
                p.get("input_preview").and_then(Value::as_str).unwrap_or("").to_string();
            let _ = evt_tx
                .send(McpEvent::PermissionRequest {
                    request_id,
                    tool_name,
                    description,
                    input_preview,
                })
                .await;
        }
        _ => {}
    }
}
