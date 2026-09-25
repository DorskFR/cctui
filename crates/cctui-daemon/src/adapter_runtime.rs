//! Runtime side of the adapter contract.
//!
//! The wire types ([`cctui_proto::adapter::AdapterEvent`] and friends) live
//! in `cctui-proto` and stay runtime-free. The `Adapter` trait here adds
//! the async/tokio surface — adapters compiled into the daemon implement
//! this trait, and the supervisor drives them.

use std::collections::BTreeMap;

use cctui_proto::adapter::{
    AdapterCommand, AdapterEvent, ForkExtract, RemoveInitiator, SessionSpec,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Per-adapter execution context handed to [`Adapter::start`].
pub struct AdapterCtx {
    /// Outbound: adapter pushes events here; the daemon multiplexes them
    /// into the server WS.
    pub events: mpsc::Sender<AdapterEvent>,
    /// Inbound: daemon pushes commands targeting this adapter here.
    pub commands: mpsc::Receiver<AdapterCommand>,
    /// Daemon-wide shutdown signal. Adapters MUST observe it and return
    /// cleanly when it fires.
    pub shutdown: CancellationToken,
    /// Adapter-specific declarative config from `adapters_enabled.config`.
    pub config: serde_json::Value,
    /// Authenticated client back to the cctui-server. Lets an adapter
    /// pull launch-time data the server owns — currently the per-session gateway
    /// env resolved from `sessions.account_id`. `None` outside a real daemon run
    /// (tests construct ctx without a server).
    pub server: Option<crate::client::ServerClient>,
    /// The daemon's machine key, paired with `server` for authenticated pulls.
    pub machine_key: Option<String>,
    /// Fires once per established server connection, including the first.
    /// An adapter owning live sessions must re-announce them on each edge: the
    /// server re-applies `daemon_lost` on every WS drop and only a fresh
    /// `SessionStarted` clears it. Carries no payload.
    pub connected: tokio::sync::broadcast::Receiver<()>,
}

#[async_trait::async_trait]
pub trait Adapter: Send + Sync {
    fn id(&self) -> &'static str;
    async fn start(&self, ctx: AdapterCtx) -> anyhow::Result<()>;
}

/// Compile-time-registered adapter factory.
pub trait AdapterFactory: Send + Sync {
    fn id(&self) -> &'static str;
    fn build(&self, config: serde_json::Value) -> Box<dyn Adapter>;
}

/// How a [`SessionDriver`] answered a command.
pub enum Handled {
    /// The loop reports success for a correlated command.
    Done,
    /// The driver reports the outcome of a correlated command itself.
    Deferred,
}

/// The error every [`SessionDriver`] method an adapter does not implement
/// returns.
#[derive(Debug)]
pub struct Unsupported(pub &'static str);

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} is not supported by this adapter", self.0)
    }
}

impl std::error::Error for Unsupported {}

pub type CommandOutcome = anyhow::Result<Handled>;

fn unsupported(command: &'static str) -> CommandOutcome {
    Err(Unsupported(command).into())
}

/// One method per [`AdapterCommand`] variant, dispatched by
/// [`dispatch_command`]. An `Err` becomes a failed `CommandResult` for a
/// correlated command.
#[async_trait::async_trait]
pub trait SessionDriver: Send {
    fn adapter_id(&self) -> &'static str;

    async fn spawn(
        &mut self,
        spec: SessionSpec,
        command_id: Option<Uuid>,
        session_id: Option<Uuid>,
    ) -> CommandOutcome;

    async fn send_message(&mut self, local_id: String, text: String) -> CommandOutcome;

    async fn reply(
        &mut self,
        local_id: String,
        text: String,
        ask_picks: Option<Vec<Vec<usize>>>,
        env: BTreeMap<String, String>,
        command_id: Option<Uuid>,
        turn_id: Option<Uuid>,
    ) -> CommandOutcome;

    async fn kill(&mut self, local_id: String, signal: Option<i32>) -> CommandOutcome;

    async fn resume_marks(&mut self, _marks: Vec<(String, u64)>) -> CommandOutcome {
        unsupported("resume_marks")
    }

    async fn fork(
        &mut self,
        _parent_local_id: String,
        _spec: SessionSpec,
        _command_id: Option<Uuid>,
        _session_id: Option<String>,
        _extract: Option<ForkExtract>,
    ) -> CommandOutcome {
        unsupported("fork")
    }

    async fn interrupt(&mut self, _local_id: String, _command_id: Option<Uuid>) -> CommandOutcome {
        unsupported("interrupt")
    }

    async fn resume(
        &mut self,
        _local_id: String,
        _working_dir: Option<String>,
        _env: BTreeMap<String, String>,
    ) -> CommandOutcome {
        unsupported("resume")
    }

    async fn permission_response(
        &mut self,
        _local_id: String,
        _request_id: String,
        _allow: bool,
    ) -> CommandOutcome {
        unsupported("permission_response")
    }

    async fn rename(&mut self, _local_id: String, _name: String) -> CommandOutcome {
        unsupported("rename")
    }

    async fn remove(
        &mut self,
        _local_id: String,
        _command_id: Option<Uuid>,
        _initiator: RemoveInitiator,
    ) -> CommandOutcome {
        unsupported("remove")
    }

    async fn set_model(
        &mut self,
        _local_id: String,
        _model: Option<String>,
        _effort: Option<String>,
        _command_id: Option<Uuid>,
    ) -> CommandOutcome {
        unsupported("set_model")
    }

    async fn diagnose(&mut self, _local_id: String, _request_id: Uuid) -> CommandOutcome {
        unsupported("diagnose")
    }

    async fn watch_pty(&mut self, _local_id: String, _watch: bool) -> CommandOutcome {
        unsupported("watch_pty")
    }
}

/// Route one command to its driver method and report the outcome of a
/// correlated command. The only `match` over [`AdapterCommand`] the adapters
/// have; it has no wildcard arm, so a new variant needs a driver method.
pub async fn dispatch_command<D: SessionDriver + ?Sized>(
    driver: &mut D,
    events: &mpsc::Sender<AdapterEvent>,
    cmd: AdapterCommand,
) {
    let command_id = cmd.command_id();
    let outcome = match cmd {
        AdapterCommand::ResumeMarks { marks } => driver.resume_marks(marks).await,
        AdapterCommand::SendMessage { local_id, text } => driver.send_message(local_id, text).await,
        AdapterCommand::Kill { local_id, signal } => driver.kill(local_id, signal).await,
        AdapterCommand::Spawn { spec, command_id, session_id } => {
            driver.spawn(spec, command_id, session_id).await
        }
        AdapterCommand::Fork { parent_local_id, spec, command_id, session_id, extract } => {
            driver.fork(parent_local_id, spec, command_id, session_id, extract).await
        }
        AdapterCommand::Reply { local_id, text, ask_picks, env, command_id, turn_id } => {
            driver.reply(local_id, text, ask_picks, env, command_id, turn_id).await
        }
        AdapterCommand::Interrupt { local_id, command_id } => {
            driver.interrupt(local_id, command_id).await
        }
        AdapterCommand::Resume { local_id, working_dir, env } => {
            driver.resume(local_id, working_dir, env).await
        }
        AdapterCommand::PermissionResponse { local_id, request_id, allow } => {
            driver.permission_response(local_id, request_id, allow).await
        }
        AdapterCommand::Rename { local_id, name } => driver.rename(local_id, name).await,
        AdapterCommand::Remove { local_id, command_id, initiator } => {
            driver.remove(local_id, command_id, initiator).await
        }
        AdapterCommand::SetModel { local_id, model, effort, command_id } => {
            driver.set_model(local_id, model, effort, command_id).await
        }
        AdapterCommand::Diagnose { local_id, request_id } => {
            driver.diagnose(local_id, request_id).await
        }
        AdapterCommand::WatchPty { local_id, watch } => driver.watch_pty(local_id, watch).await,
    };
    report_outcome(driver.adapter_id(), events, command_id, outcome).await;
}

async fn report_outcome(
    adapter: &'static str,
    events: &mpsc::Sender<AdapterEvent>,
    command_id: Option<Uuid>,
    outcome: CommandOutcome,
) {
    let error = match outcome {
        Ok(Handled::Deferred) => return,
        Ok(Handled::Done) => None,
        Err(err) => {
            if err.is::<Unsupported>() {
                tracing::debug!(adapter, %err, "command not supported");
            } else {
                tracing::warn!(adapter, %err, "command dispatch failed");
            }
            Some(err.to_string())
        }
    };
    let Some(command_id) = command_id else { return };
    let _ =
        events.send(AdapterEvent::CommandResult { command_id, ok: error.is_none(), error }).await;
}

/// Feed `commands` to `driver` one at a time until shutdown fires or the
/// sender closes.
pub async fn run_command_loop<D: SessionDriver + ?Sized>(
    driver: &mut D,
    commands: &mut mpsc::Receiver<AdapterCommand>,
    events: &mpsc::Sender<AdapterEvent>,
    shutdown: &CancellationToken,
) {
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            cmd = commands.recv() => {
                let Some(cmd) = cmd else { return };
                dispatch_command(driver, events, cmd).await;
            }
        }
    }
}
