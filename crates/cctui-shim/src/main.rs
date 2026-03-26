use clap::{Parser, Subcommand};
use futures_util::SinkExt;
use tokio::io::{AsyncBufReadExt, BufReader};
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
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt().with_env_filter("cctui_shim=info").init();

    let cli = Cli::parse();
    match cli.command {
        Commands::Relay { session_id, ws_url } => {
            relay(&session_id, &ws_url).await?;
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
