use std::path::PathBuf;

use clap::{Parser, Subcommand};
use tokio_util::sync::CancellationToken;

use cctui_daemon::client::ServerClient;
use cctui_daemon::config::Config;
use cctui_daemon::supervisor::Supervisor;
use cctui_daemon::{adapters, runtime, selfupdate, service};

#[derive(Parser)]
#[command(name = "cctui-daemon", about = "Per-machine agent supervisor for cctui", version)]
struct Cli {
    #[arg(long, env = "CCTUI_DAEMON_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Enrol this machine with a cctui-server and write the resulting
    /// machine key to the config file.
    Enroll {
        #[arg(long)]
        server_url: String,
        #[arg(long)]
        token: String,
        #[arg(long)]
        name: String,
    },
    /// Connect to the configured server and supervise adapters.
    Run {
        /// Disable the periodic auto-update poller. Equivalent to
        /// `CCTUI_DAEMON_AUTOUPDATE=0`.
        #[arg(long)]
        no_auto_update: bool,
    },
    /// Print the resolved configuration (`machine_key` redacted).
    Status,
    /// Check for a newer release, swap the binary in place, and restart
    /// the daemon service (if one is running) so it picks up the new binary.
    Update,
    /// Install / uninstall the cctui-daemon systemd user service.
    Service {
        #[command(subcommand)]
        cmd: ServiceCmd,
    },
}

#[derive(Subcommand)]
enum ServiceCmd {
    /// Write the user-systemd unit, reload, enable, and start it.
    Install,
    /// Stop + disable the unit and remove the user-systemd file.
    Uninstall,
    /// Restart the running cctui-daemon service.
    Restart,
    /// Show the service manager status and the most recent daemon logs.
    Status,
    /// Print the embedded unit content to stdout (for manual install).
    Unit,
}

/// Print resolved config plus the version split: the binary this CLI is, and
/// the version of the service actually running (from the runtime state file).
fn print_status(path: &PathBuf) -> anyhow::Result<()> {
    if !Config::exists_at(path) {
        println!("config: {} (not found)", path.display());
        println!(
            "enrolled: no — run `cctui-daemon enroll --server-url <url> --token <token> --name <name>`"
        );
        println!("service: {}", if service::is_active() { "running" } else { "not running" });
        println!("binary version: {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cfg = Config::load_from(path)?;
    println!("config: {}", path.display());
    println!("server_url: {}", cfg.server_url);
    if let Some(id) = cfg.machine_id {
        println!("machine_id: {id}");
    }
    println!("machine_key: <redacted>");
    println!("service: {}", if service::is_active() { "running" } else { "not running" });
    println!("binary version: {}", env!("CARGO_PKG_VERSION"));
    match runtime::read() {
        Some(rt) if runtime::pid_alive(rt.pid) => {
            println!("running version: {} (pid {}, since {})", rt.version, rt.pid, rt.started_at);
        }
        Some(rt) => println!(
            "running version: none — last run was {} (pid {}, since {}, no longer alive)",
            rt.version, rt.pid, rt.started_at
        ),
        None => println!("running version: unknown (daemon has not run on this machine)"),
    }
    Ok(())
}

fn auto_update_enabled(flag: bool) -> bool {
    if flag {
        return false;
    }
    !matches!(std::env::var("CCTUI_DAEMON_AUTOUPDATE").as_deref(), Ok("0" | "false"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "cctui_daemon=info".into()),
        )
        .init();

    let cli = Cli::parse();
    let path = cli.config.unwrap_or_else(Config::default_path);

    match cli.cmd {
        Cmd::Enroll { server_url, token, name } => {
            let client = ServerClient::new(&server_url);
            let resp = client.enroll(&token, &name).await?;
            let cfg = Config {
                server_url,
                machine_key: resp.machine_key,
                machine_id: Some(resp.machine_id),
            };
            cfg.save_to(&path)?;
            println!("enrolled as {} → {}", resp.machine_id, path.display());
            Ok(())
        }
        Cmd::Run { no_auto_update } => {
            let cfg = Config::load_from(&path)?;
            // Record this process as the running service so `status` /
            // `service status` can report the version actually serving.
            runtime::record();
            let client = ServerClient::new(&cfg.server_url);
            // Confirm identity once up-front so misconfigurations fail loudly.
            let auth = client.daemon_auth(&cfg.machine_key).await?;
            tracing::info!(machine_id = %auth.machine_id, user_id = %auth.user_id, "authenticated");
            // Captured for the self-update loop before `machine_key` is moved
            // into the supervisor; both flow to the server-routed updater.
            let update_server_url = cfg.server_url.clone();
            let update_machine_key = cfg.machine_key.clone();
            let supervisor = Supervisor::new(client, cfg.machine_key, adapters::registry());
            let shutdown = CancellationToken::new();
            // Translate Ctrl-C / SIGTERM into shutdown.
            let signal_token = shutdown.clone();
            tokio::spawn(async move {
                let _ = tokio::signal::ctrl_c().await;
                signal_token.cancel();
            });
            if auto_update_enabled(no_auto_update) {
                let interval = selfupdate::poll_interval();
                tracing::info!(interval_secs = interval.as_secs(), "auto-update enabled");
                selfupdate::spawn_loop(
                    shutdown.clone(),
                    update_server_url,
                    update_machine_key,
                    interval,
                );
            } else {
                tracing::info!("auto-update disabled");
            }
            supervisor.run(shutdown).await;
            Ok(())
        }
        Cmd::Update => {
            let cfg = Config::load_from(&path)?;
            match selfupdate::check_and_apply(&cfg.server_url, &cfg.machine_key).await {
                Ok(Some(_)) => {
                    // The running service is a separate process still on the
                    // old binary — restart it so the swap takes effect now.
                    match service::restart_if_active() {
                        Ok(true) => println!("cctui-daemon upgraded and service restarted"),
                        Ok(false) => println!(
                            "cctui-daemon upgraded; no running service found — \
                             start it (`cctui-daemon service install`) or restart to apply"
                        ),
                        Err(err) => println!(
                            "cctui-daemon upgraded, but restarting the service failed: {err}\n\
                             restart it manually (`cctui-daemon service restart`) to apply"
                        ),
                    }
                    Ok(())
                }
                Ok(None) => {
                    println!("cctui-daemon already on the latest release");
                    Ok(())
                }
                Err(err) => Err(err),
            }
        }
        Cmd::Service { cmd } => match cmd {
            ServiceCmd::Install => service::install(),
            ServiceCmd::Uninstall => service::uninstall(),
            ServiceCmd::Restart => service::restart(),
            ServiceCmd::Status => service::status(),
            ServiceCmd::Unit => {
                service::print_unit();
                Ok(())
            }
        },
        Cmd::Status => print_status(&path),
    }
}
