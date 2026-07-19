use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

use cctui_dispatcher_kube::client::ServerClient;
use cctui_dispatcher_kube::config::Config;
use cctui_dispatcher_kube::run::Runner;
use cctui_dispatcher_kube::spawn::Spawner;

#[derive(Parser)]
#[command(
    name = "cctui-dispatcher-kube",
    about = "Standalone kubernetes dispatcher: enrolls to an account and spawns worker Jobs in-cluster on dispatch",
    version
)]
struct Cli {
    #[arg(long, env = "CCTUI_DISPATCHER_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enroll this dispatcher with a cctui-server and write the resulting key
    /// to the config file.
    Enroll {
        #[arg(long)]
        server_url: String,
        #[arg(long)]
        token: String,
        #[arg(long)]
        name: String,
        /// Namespace the worker Job + its `WorkerProfile` resources live in.
        #[arg(long)]
        namespace: String,
        /// `WorkerProfile` instantiated when a dispatch selects none by name.
        #[arg(long)]
        default_profile: String,
        /// `CCTUI_URL` injected into spawned workers (defaults to `server_url`).
        #[arg(long)]
        worker_cctui_url: Option<String>,
        /// OAuth account name to bind as this dispatcher's default (CCT-427).
        /// A dispatch with no explicit account routes its model traffic through
        /// the cctui gateway under this account.
        #[arg(long)]
        account: Option<String>,
        /// Provider hint disambiguating an account name shared across providers
        /// (e.g. `anthropic` vs `openai`). Only meaningful with `--account`.
        #[arg(long)]
        provider: Option<String>,
    },
    /// Connect to the configured server and serve dispatch commands.
    Run,
    /// Print the resolved configuration (`dispatcher_key` redacted).
    Status,
}

fn print_status(path: &Path) -> anyhow::Result<()> {
    if !Config::exists_at(path) {
        println!("config: {} (not found)", path.display());
        println!(
            "enrolled: no — run `cctui-dispatcher-kube enroll --server-url <url> \
             --token <token> --name <name> --namespace <ns> --default-profile <name>`"
        );
        println!("binary version: {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cfg = Config::load_from(path)?;
    println!("config: {}", path.display());
    println!("server_url: {}", cfg.server_url);
    if let Some(id) = cfg.dispatcher_id {
        println!("dispatcher_id: {id}");
    }
    println!("dispatcher_key: <redacted>");
    println!("namespace: {}", cfg.namespace);
    println!("default_profile: {}", cfg.default_profile);
    println!("worker CCTUI_URL: {}", cfg.worker_url());
    println!("binary version: {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("rustls crypto provider already installed"))?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_dispatcher_kube=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(Config::default_path);

    match cli.cmd {
        Cmd::Enroll {
            server_url,
            token,
            name,
            namespace,
            default_profile,
            worker_cctui_url,
            account,
            provider,
        } => {
            let client = ServerClient::new(&server_url);
            let resp =
                client.enroll(&token, &name, account.as_deref(), provider.as_deref()).await?;
            let cfg = Config {
                server_url,
                dispatcher_key: resp.dispatcher_key,
                dispatcher_id: Some(resp.dispatcher_id),
                namespace,
                default_profile,
                worker_cctui_url,
            };
            cfg.save_to(&path)?;
            println!("enrolled as {} → {}", resp.dispatcher_id, path.display());
            Ok(())
        }
        Cmd::Run => {
            let cfg = Config::load_from(&path)?;
            let client = ServerClient::new(&cfg.server_url);
            // Confirm identity up-front so misconfigurations fail loudly.
            let auth = client.dispatcher_auth(&cfg.dispatcher_key).await?;
            tracing::info!(user_id = %auth.user_id, "authenticated");
            let spawner = Spawner::connect(
                cfg.namespace.clone(),
                cfg.default_profile.clone(),
                cfg.worker_url().to_owned(),
            )
            .await?;
            let runner = Runner::new(client, cfg.dispatcher_key, spawner);
            let shutdown = CancellationToken::new();
            let signal_token = shutdown.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                signal_token.cancel();
            });
            runner.run(shutdown).await;
            Ok(())
        }
        Cmd::Status => print_status(&path),
    }
}
