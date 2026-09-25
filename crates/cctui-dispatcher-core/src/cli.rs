//! The `enroll` / `run` / `status` binary shared by every dispatcher.
//!
//! A platform crate implements [`Backend`] and its `main` is
//! `cctui_dispatcher_core::cli::main::<MyBackend>().await`.

use std::ffi::OsString;
use std::future::Future;
use std::path::{Path, PathBuf};

use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use tokio_util::sync::CancellationToken;

use crate::{Dispatcher, DispatcherConfig, Runner, ServerClient};

pub trait Backend {
    /// Binary name, e.g. `cctui-dispatcher-docker`.
    const NAME: &'static str;
    /// Dispatcher kind sent on enroll, e.g. `docker`.
    const KIND: &'static str;
    const ABOUT: &'static str;

    type Config: DispatcherConfig;
    type EnrollArgs: Args;
    type Spawner: Dispatcher;

    fn config(args: Self::EnrollArgs, enrolled: Enrolled) -> Self::Config;

    /// Build the spawner for `run`. Background tasks it starts must stop when
    /// `shutdown` fires.
    fn spawner(
        cfg: &Self::Config,
        shutdown: &CancellationToken,
    ) -> impl Future<Output = anyhow::Result<Self::Spawner>>;

    /// Backend-specific `status` lines, printed after the common ones.
    fn status_lines(cfg: &Self::Config) -> Vec<(&'static str, String)>;
}

/// The part of a config every backend gets from `enroll`.
#[derive(Debug, Clone)]
pub struct Enrolled {
    pub server_url: String,
    pub dispatcher_key: String,
    pub dispatcher_id: uuid::Uuid,
    pub worker_cctui_url: Option<String>,
}

#[derive(Parser, Debug)]
#[command(version)]
pub struct Cli<E: Args> {
    #[arg(long, env = "CCTUI_DISPATCHER_CONFIG")]
    pub config: Option<PathBuf>,
    #[command(subcommand)]
    pub cmd: Cmd<E>,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
pub enum Cmd<E: Args> {
    /// Enroll this dispatcher with a cctui-server and write the resulting key
    /// to the config file.
    Enroll {
        #[command(flatten)]
        common: EnrollCommon,
        #[command(flatten)]
        backend: E,
    },
    /// Connect to the configured server and serve dispatch commands.
    Run,
    /// Print the resolved configuration (`dispatcher_key` redacted).
    Status,
}

#[derive(Args, Debug)]
pub struct EnrollCommon {
    #[arg(long)]
    pub server_url: String,
    #[arg(long)]
    pub token: String,
    #[arg(long)]
    pub name: String,
    /// `CCTUI_URL` injected into spawned workers (defaults to `server_url`).
    #[arg(long)]
    pub worker_cctui_url: Option<String>,
    /// OAuth account name to bind as this dispatcher's default. A dispatch
    /// with no explicit account routes its model traffic through the cctui
    /// gateway under this account.
    #[arg(long)]
    pub account: Option<String>,
    /// Provider hint disambiguating an account name shared across providers
    /// (e.g. `anthropic` vs `openai`). Only meaningful with `--account`.
    #[arg(long)]
    pub provider: Option<String>,
}

#[must_use]
pub fn command<B: Backend>() -> clap::Command {
    Cli::<B::EnrollArgs>::command().name(B::NAME).bin_name(B::NAME).about(B::ABOUT)
}

pub fn try_parse_from<B, I, T>(args: I) -> Result<Cli<B::EnrollArgs>, clap::Error>
where
    B: Backend,
    I: IntoIterator<Item = T>,
    T: Into<OsString> + Clone,
{
    let matches = command::<B>().try_get_matches_from(args)?;
    Cli::from_arg_matches(&matches)
}

/// Resolves on SIGTERM (docker stop, launchd, kubernetes) or Ctrl-C. The
/// handlers are installed when this is called, not when it is first polled.
pub fn shutdown_signal() -> std::io::Result<impl Future<Output = ()>> {
    #[cfg(unix)]
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    Ok(async move {
        #[cfg(unix)]
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        #[cfg(not(unix))]
        let _ = tokio::signal::ctrl_c().await;
    })
}

pub async fn main<B: Backend>() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let own_target = B::NAME.replace('-', "_");
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| format!("{own_target}=info,cctui_dispatcher_core=info").into()),
        )
        .init();

    let cli = try_parse_from::<B, _, _>(std::env::args_os()).unwrap_or_else(|e| e.exit());
    let path = cli.config.unwrap_or_else(B::Config::default_path);

    match cli.cmd {
        Cmd::Enroll { common, backend } => enroll::<B>(common, backend, &path).await,
        Cmd::Run => run::<B>(&path).await,
        Cmd::Status => print_status::<B>(&path),
    }
}

async fn enroll<B: Backend>(
    common: EnrollCommon,
    backend: B::EnrollArgs,
    path: &Path,
) -> anyhow::Result<()> {
    let client = ServerClient::new(&common.server_url, B::KIND);
    let resp = client
        .enroll(&common.token, &common.name, common.account.as_deref(), common.provider.as_deref())
        .await?;
    let cfg = B::config(
        backend,
        Enrolled {
            server_url: common.server_url,
            dispatcher_key: resp.dispatcher_key,
            dispatcher_id: resp.dispatcher_id,
            worker_cctui_url: common.worker_cctui_url,
        },
    );
    cfg.save_to(path)?;
    println!("enrolled as {} → {}", resp.dispatcher_id, path.display());
    Ok(())
}

async fn run<B: Backend>(path: &Path) -> anyhow::Result<()> {
    let cfg = B::Config::load_from(path)?;
    let client = ServerClient::new(cfg.server_url(), B::KIND);
    let auth = client.dispatcher_auth(cfg.dispatcher_key()).await?;
    tracing::info!(user_id = %auth.user_id, "authenticated");
    let shutdown = CancellationToken::new();
    let signal = shutdown_signal()?;
    let spawner = B::spawner(&cfg, &shutdown).await?;
    let runner = Runner::new(client, cfg.dispatcher_key().to_owned(), spawner);
    let signal_token = shutdown.clone();
    tokio::spawn(async move {
        signal.await;
        tracing::info!("shutdown signal received; draining in-flight dispatches");
        signal_token.cancel();
    });
    runner.run(shutdown).await;
    Ok(())
}

fn print_status<B: Backend>(path: &Path) -> anyhow::Result<()> {
    if !B::Config::exists_at(path) {
        println!("config: {} (not found)", path.display());
        println!("enrolled: no — run `{}`", B::Config::ENROLL_HINT);
        println!("binary version: {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let cfg = B::Config::load_from(path)?;
    println!("config: {}", path.display());
    println!("server_url: {}", cfg.server_url());
    if let Some(id) = cfg.dispatcher_id() {
        println!("dispatcher_id: {id}");
    }
    println!("dispatcher_key: <redacted>");
    for (key, value) in B::status_lines(&cfg) {
        println!("{key}: {value}");
    }
    println!("worker CCTUI_URL: {}", cfg.worker_url());
    println!("binary version: {}", env!("CARGO_PKG_VERSION"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn sigterm_resolves_shutdown_signal() {
        let signal = shutdown_signal().unwrap();
        let status = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .unwrap();
        assert!(status.success());
        tokio::time::timeout(std::time::Duration::from_secs(5), signal)
            .await
            .expect("SIGTERM did not resolve shutdown_signal");
    }
}
