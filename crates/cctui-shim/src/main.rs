use clap::{Parser, Subcommand};
use futures_util::SinkExt;
use serde::Serialize;
use tokio::fs::File;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

#[derive(Parser)]
#[command(name = "cctui-shim", about = "Claude Code stream-json WebSocket relay")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Relay stdin (stream-json JSONL) to the server via WebSocket
    Relay {
        /// Session ID assigned during registration
        #[arg(long)]
        session_id: String,
        /// WebSocket URL to connect to
        #[arg(long)]
        ws_url: String,
    },
    /// Register a Claude session with the server
    Register {
        /// cctui server URL
        #[arg(long, env = "CCTUI_URL", default_value = "http://localhost:8700")]
        server_url: String,
        /// Agent auth token
        #[arg(long, env = "CCTUI_TOKEN", default_value = "dev-agent")]
        token: String,
        /// Machine identifier
        #[arg(long, env = "CCTUI_MACHINE_ID", default_value = "local")]
        machine_id: String,
        /// Path to the Claude transcript file (optional, for streaming)
        #[arg(long)]
        transcript: Option<String>,
    },
    /// Tail a Claude transcript JSONL file and POST events to the server
    Stream {
        /// Session ID
        #[arg(long)]
        session_id: String,
        /// Path to the transcript JSONL file
        #[arg(long)]
        transcript: String,
        /// cctui server URL
        #[arg(long, env = "CCTUI_URL", default_value = "http://localhost:8700")]
        server_url: String,
        /// Agent auth token
        #[arg(long, env = "CCTUI_TOKEN", default_value = "dev-agent")]
        token: String,
    },
}

#[derive(Serialize, Default)]
struct StreamerEvent {
    #[serde(rename = "type")]
    event_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    input: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_use_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tokens_out: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cost_usd: Option<f64>,
    ts: i64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("cctui_shim=info").init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Relay { session_id, ws_url } => {
            relay(&session_id, &ws_url).await?;
        }
        Commands::Register { server_url, token, machine_id, transcript } => {
            register(&server_url, &token, &machine_id, transcript.as_deref()).await?;
        }
        Commands::Stream { session_id, transcript, server_url, token } => {
            stream(&session_id, &transcript, &server_url, &token).await?;
        }
    }
    Ok(())
}

#[allow(clippy::cognitive_complexity)]
async fn relay(session_id: &str, ws_url: &str) -> anyhow::Result<()> {
    tracing::info!(session_id, ws_url, "connecting to server");

    let (ws_stream, _) = connect_async(ws_url).await?;
    let (mut ws_tx, _ws_rx) = futures_util::StreamExt::split(ws_stream);

    tracing::info!("connected, relaying stdin");

    let stdin = tokio::io::stdin();
    let reader = BufReader::new(stdin);
    let mut lines = reader.lines();

    while let Ok(Some(line)) = lines.next_line().await {
        if serde_json::from_str::<serde_json::Value>(&line).is_ok()
            && ws_tx.send(Message::Text(line.clone().into())).await.is_err()
        {
            tracing::error!("WebSocket send failed");
            break;
        }
        println!("{line}");
    }

    tracing::info!("stdin closed, shutting down");
    Ok(())
}

#[allow(clippy::cognitive_complexity)]
async fn register(
    server_url: &str,
    token: &str,
    machine_id: &str,
    transcript: Option<&str>,
) -> anyhow::Result<()> {
    // Read JSON from stdin
    let mut stdin_data = String::new();
    tokio::io::stdin().read_to_string(&mut stdin_data).await?;

    let hook_data: serde_json::Value =
        serde_json::from_str(&stdin_data).unwrap_or_else(|_| serde_json::json!({}));

    let session_id = hook_data.get("session_id").and_then(|v| v.as_str()).unwrap_or("");

    let cwd = hook_data.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");

    let model = hook_data.get("model").and_then(|v| v.as_str()).unwrap_or("");

    let transcript_path = hook_data.get("transcript_path").and_then(|v| v.as_str());

    // Build git branch if possible
    let git_branch = tokio::process::Command::new("git")
        .args(["-C", cwd, "rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .await
        .ok()
        .and_then(|output| {
            if output.status.success() {
                String::from_utf8(output.stdout).ok().map(|s| s.trim().to_string())
            } else {
                None
            }
        })
        .unwrap_or_else(|| "none".to_string());

    let metadata = serde_json::json!({
        "git_branch": git_branch,
        "project_name": std::path::Path::new(cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown"),
        "model": model,
        "transcript_path": transcript_path,
    });

    let register_req = serde_json::json!({
        "claude_session_id": session_id,
        "machine_id": machine_id,
        "working_dir": cwd,
        "metadata": metadata,
    });

    let client = reqwest::Client::new();
    let register_url = format!("{server_url}/api/v1/sessions/register");

    match client
        .post(&register_url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&register_req)
        .send()
        .await
    {
        Ok(resp) => {
            if let Ok(body) = resp.json::<serde_json::Value>().await {
                if let Some(sid) = body.get("session_id") {
                    println!("{sid}");
                }
                if let Some(ws_url) = body.get("ws_url") {
                    println!("{ws_url}");
                }
            }
            tracing::info!("registered session: {session_id}");
        }
        Err(e) => {
            tracing::error!("registration failed: {e}");
        }
    }

    // If transcript path is provided, spawn stream task
    if let Some(tx_path) = transcript.or(transcript_path) {
        tracing::info!("starting stream task for {tx_path}");
        stream(session_id, tx_path, server_url, token).await?;
    }

    Ok(())
}

#[allow(clippy::cognitive_complexity)]
async fn stream(
    session_id: &str,
    transcript_path: &str,
    server_url: &str,
    token: &str,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let api_url = format!("{server_url}/api/v1/events/{session_id}");

    // Wait for transcript file to appear (up to 30 seconds)
    let mut file = None;
    for _ in 0..60 {
        match File::open(transcript_path).await {
            Ok(f) => {
                file = Some(f);
                break;
            }
            Err(_) => {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
            }
        }
    }

    let Some(mut file) = file else {
        tracing::warn!("transcript file never appeared: {transcript_path}");
        return Ok(());
    };

    let mut reader = BufReader::new(&mut file);
    let mut line = String::new();

    // Process existing lines
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                let events = parse_transcript_line(&line);
                for event in events {
                    post_event(&client, &api_url, token, event).await;
                }
            }
            Err(e) => {
                tracing::error!("read error: {e}");
                break;
            }
        }
    }

    // Tail for new lines
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                // No new data, check if session is still alive
                if !is_session_active(session_id).await {
                    tracing::info!("session ended, exiting stream");
                    break;
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
            }
            Ok(_) => {
                let events = parse_transcript_line(&line);
                for event in events {
                    post_event(&client, &api_url, token, event).await;
                }
            }
            Err(e) => {
                tracing::error!("read error: {e}");
                break;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_lines)]
fn parse_transcript_line(line: &str) -> Vec<StreamerEvent> {
    let Ok(d) = serde_json::from_str::<serde_json::Value>(line) else {
        return vec![];
    };

    let msg = d.get("message").cloned().unwrap_or_default();
    let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
    let content = msg.get("content");
    let msg_type = d.get("type").and_then(|v| v.as_str()).unwrap_or("");

    // Skip non-conversation events
    if matches!(msg_type, "file-history-snapshot" | "queue-operation" | "system") {
        return vec![];
    }
    if role == "system" {
        return vec![];
    }

    let ts = chrono::Utc::now().timestamp();
    let mut events = Vec::new();

    if role == "user" {
        if let Some(serde_json::Value::String(s)) = content {
            if !s.is_empty() {
                events.push(StreamerEvent {
                    event_type: "user_message".into(),
                    content: Some(s.clone()),
                    ts,
                    ..Default::default()
                });
            }
        } else if let Some(serde_json::Value::Array(parts)) = content {
            for part in parts {
                let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match part_type {
                    "tool_result" => {
                        let tool_use_id = part
                            .get("tool_use_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let content_str = part
                            .get("content")
                            .map(std::string::ToString::to_string)
                            .unwrap_or_default();
                        let truncated = content_str.chars().take(500).collect::<String>();
                        events.push(StreamerEvent {
                            event_type: "tool_result".into(),
                            tool_use_id: Some(tool_use_id),
                            content: Some(truncated),
                            ts,
                            ..Default::default()
                        });
                    }
                    "text" => {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str())
                            && !text.is_empty()
                        {
                            events.push(StreamerEvent {
                                event_type: "user_message".to_string(),
                                content: Some(text.to_string()),
                                ts,
                                ..Default::default()
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    } else if role == "assistant" {
        if let Some(serde_json::Value::Array(parts)) = content {
            for part in parts {
                let part_type = part.get("type").and_then(|v| v.as_str()).unwrap_or("");
                match part_type {
                    "text" => {
                        if let Some(text) = part.get("text").and_then(|v| v.as_str())
                            && !text.is_empty()
                        {
                            events.push(StreamerEvent {
                                event_type: "assistant_message".to_string(),
                                content: Some(text.to_string()),
                                ts,
                                ..Default::default()
                            });
                        }
                    }
                    "tool_use" => {
                        let tool =
                            part.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                        let input = part.get("input").cloned().unwrap_or_default();
                        events.push(StreamerEvent {
                            event_type: "tool_call".into(),
                            tool: Some(tool),
                            input: Some(input),
                            ts,
                            ..Default::default()
                        });
                    }
                    _ => {}
                }
            }
        }

        // Check for usage in the message
        if let Some(usage) = msg.get("usage") {
            let tokens_in =
                usage.get("input_tokens").and_then(serde_json::Value::as_u64).unwrap_or(0)
                    + usage
                        .get("cache_creation_input_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                    + usage
                        .get("cache_read_input_tokens")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0);
            let tokens_out =
                usage.get("output_tokens").and_then(serde_json::Value::as_u64).unwrap_or(0);
            let cost_usd = (tokens_in as f64 / 1_000_000.0)
                .mul_add(3.0, (tokens_out as f64 / 1_000_000.0) * 15.0);
            if tokens_in > 0 || tokens_out > 0 {
                events.push(StreamerEvent {
                    event_type: "usage".into(),
                    tokens_in: Some(tokens_in),
                    tokens_out: Some(tokens_out),
                    cost_usd: Some(cost_usd),
                    ts,
                    ..Default::default()
                });
            }
        }
    }

    events
}

async fn post_event(client: &reqwest::Client, api_url: &str, token: &str, event: StreamerEvent) {
    let _ = client
        .post(api_url)
        .header("Authorization", format!("Bearer {token}"))
        .json(&event)
        .send()
        .await;
}

async fn is_session_active(session_id: &str) -> bool {
    if let Ok(home) = std::env::var("HOME") {
        let pid_file = std::path::PathBuf::from(format!("{home}/.cctui/session_id"));
        if let Ok(content) = tokio::fs::read_to_string(&pid_file).await {
            let current_sid = content.trim();
            return current_sid == session_id;
        }
    }
    true // assume active if we can't check
}
