use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

use cctui_dispatcher_apple::cli::RealCli;
use cctui_dispatcher_apple::client::ServerClient;
use cctui_dispatcher_apple::config::Config;
use cctui_dispatcher_apple::run::Runner;
use cctui_dispatcher_apple::spawn::Spawner;

#[derive(Parser)]
#[command(
    name = "cctui-dispatcher-apple",
    about = "Standalone Apple `container` dispatcher: enrolls to an account and boots worker micro-VMs on dispatch",
    version
)]
struct Cli {
    #[arg(long, env = "CCTUI_DISPATCHER_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
enum Cmd {
    /// Enroll this dispatcher with a cctui-server and write the resulting key to
    /// the config file.
    Enroll {
        #[arg(long)]
        server_url: String,
        #[arg(long)]
        token: String,
        #[arg(long)]
        name: String,
        /// Worker OCI image to boot on dispatch.
        #[arg(long)]
        image: String,
        /// `CCTUI_URL` injected into spawned workers (defaults to `server_url`).
        #[arg(long)]
        worker_cctui_url: Option<String>,
        /// Container network to attach spawned micro-VMs to.
        #[arg(long)]
        network: Option<String>,
        /// Path to the `container` binary (defaults to `container` on `PATH`).
        #[arg(long)]
        container_bin: Option<String>,
        /// Extra bind mount(s) for spawned containers (`host:guest[:ro]`).
        #[arg(long)]
        mount: Vec<String>,
        /// Optional repo mount (`host:guest`) — shallow-pulled at boot.
        #[arg(long)]
        repo_mount: Option<String>,
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
            "enrolled: no — run `cctui-dispatcher-apple enroll --server-url <url> \
             --token <token> --name <name> --image <image>`"
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
    println!("image: {}", cfg.image);
    println!("container binary: {}", cfg.container_bin());
    println!("worker CCTUI_URL: {}", cfg.worker_url());
    println!("binary version: {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_dispatcher_apple=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(Config::default_path);

    match cli.cmd {
        Cmd::Enroll {
            server_url,
            token,
            name,
            image,
            worker_cctui_url,
            network,
            container_bin,
            mount,
            repo_mount,
        } => {
            let client = ServerClient::new(&server_url);
            let resp = client.enroll(&token, &name).await?;
            let cfg = Config {
                server_url,
                dispatcher_key: resp.dispatcher_key,
                dispatcher_id: Some(resp.dispatcher_id),
                image,
                worker_cctui_url,
                network,
                container_bin,
                mounts: mount,
                repo_mount,
                secret_mount_path: None,
                secret_dir: None,
                secret_via_env: false,
            };
            cfg.save_to(&path)?;
            println!("enrolled as {} → {}", resp.dispatcher_id, path.display());
            Ok(())
        }
        Cmd::Run => {
            let cfg = Config::load_from(&path)?;
            let client = ServerClient::new(&cfg.server_url);
            let auth = client.dispatcher_auth(&cfg.dispatcher_key).await?;
            tracing::info!(user_id = %auth.user_id, "authenticated");
            let spawner = Spawner::new(
                RealCli::new(cfg.container_bin().to_owned()),
                cfg.image.clone(),
                cfg.network.clone(),
                cfg.worker_url().to_owned(),
                cfg.mounts.clone(),
                cfg.repo_mount.clone(),
                cfg.secret_mount_path().to_owned(),
                cfg.secret_dir(),
                cfg.secret_via_env,
            );
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
