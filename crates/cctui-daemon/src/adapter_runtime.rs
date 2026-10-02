//! Runtime side of the adapter contract.
//!
//! The wire types ([`cctui_proto::adapter::AdapterEvent`] and friends) live
//! in `cctui-proto` and stay runtime-free. The `Adapter` trait here adds
//! the async/tokio surface — adapters compiled into the daemon implement
//! this trait, and the supervisor drives them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use cctui_proto::adapter::{
    AdapterCommand, AdapterEvent, ForkExtract, RemoveInitiator, SessionSpec,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A `WatchPty` routed off the command path: `(local_id, watch)`.
pub type PtyWatch = (String, bool);

const MAX_COALESCED_IDS: usize = 16;

const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PendingInterrupt {
    pub local_id: String,
    pub command_ids: Vec<Uuid>,
}

/// Interrupts coalesced per session. `push` never blocks or fails: Stop must
/// not depend on a command queue having room.
#[derive(Clone, Default)]
pub struct InterruptQueue {
    inner: Arc<InterruptState>,
}

#[derive(Default)]
struct InterruptState {
    pending: std::sync::Mutex<Vec<PendingInterrupt>>,
    notify: tokio::sync::Notify,
}

impl InterruptQueue {
    pub fn push(&self, local_id: &str, command_ids: impl IntoIterator<Item = Uuid>) {
        let ids: Vec<Uuid> = command_ids.into_iter().collect();
        let mut pending =
            self.inner.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let i = pending.iter().position(|p| p.local_id == local_id).unwrap_or_else(|| {
            pending.push(PendingInterrupt { local_id: local_id.to_owned(), command_ids: vec![] });
            pending.len() - 1
        });
        let kept = &mut pending[i].command_ids;
        let room = MAX_COALESCED_IDS.saturating_sub(kept.len());
        kept.extend(ids.into_iter().take(room));
        drop(pending);
        // Stores a permit when nobody waits yet, so a push racing ahead of
        // `next` still wakes it.
        self.inner.notify.notify_one();
    }

    /// Cancel-safe: nothing is taken until the future resolves.
    pub async fn next(&self) -> PendingInterrupt {
        loop {
            if let Some(pending) = self.try_next() {
                return pending;
            }
            self.inner.notify.notified().await;
        }
    }

    #[must_use]
    pub fn drain(&self) -> Vec<PendingInterrupt> {
        std::mem::take(
            &mut *self.inner.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
        )
    }

    #[must_use]
    pub fn same_queue(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    #[must_use]
    pub fn try_next(&self) -> Option<PendingInterrupt> {
        let mut pending =
            self.inner.pending.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        (!pending.is_empty()).then(|| pending.remove(0))
    }
}

/// Must not go through the adapter's serial command loop.
#[async_trait::async_trait]
pub trait Interrupter: Send + Sync + 'static {
    /// `Handled::Deferred` means the interrupter answers `command_ids` itself.
    async fn interrupt(&self, local_id: &str, command_ids: &[Uuid]) -> CommandOutcome;
}

/// One task per interrupt, so a slow one holds up no other session.
pub fn spawn_interrupt_pump(
    adapter: &'static str,
    queue: InterruptQueue,
    interrupter: Arc<dyn Interrupter>,
    events: mpsc::Sender<AdapterEvent>,
    shutdown: CancellationToken,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let pending = tokio::select! {
                () = shutdown.cancelled() => return,
                pending = queue.next() => pending,
            };
            let (interrupter, events) = (Arc::clone(&interrupter), events.clone());
            tokio::spawn(async move {
                let PendingInterrupt { local_id, command_ids } = pending;
                let outcome = tokio::time::timeout(
                    INTERRUPT_TIMEOUT,
                    interrupter.interrupt(&local_id, &command_ids),
                )
                .await
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!("interrupt did not complete within {INTERRUPT_TIMEOUT:?}"))
                });
                if outcome.is_ok() {
                    tracing::info!(adapter, %local_id, "interrupt delivered off the command path");
                }
                report_outcomes(adapter, &events, &command_ids, outcome).await;
            });
        }
    })
}

/// Per-adapter execution context handed to [`Adapter::start`].
pub struct AdapterCtx {
    /// Outbound: adapter pushes events here; the daemon multiplexes them
    /// into the server WS.
    pub events: mpsc::Sender<AdapterEvent>,
    /// Inbound: daemon pushes commands targeting this adapter here.
    pub commands: mpsc::Receiver<AdapterCommand>,
    /// Inbound, out-of-band: `WatchPty` only, so a live view never queues
    /// behind a command whose socket round-trip can take 30s. `None` for
    /// adapters that declared no live view.
    pub pty_watch: Option<mpsc::Receiver<PtyWatch>>,
    /// Inbound, out-of-band: `Interrupt` only. `None` keeps interrupts on
    /// `commands`; an adapter that declared them must drain this.
    pub interrupts: Option<InterruptQueue>,
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

    /// Whether this adapter, under `config`, drains [`AdapterCtx::pty_watch`].
    /// A `false` here keeps `WatchPty` on the command path, where it answers
    /// "unsupported"; a `true` that never drains the channel leaks watches.
    fn pty_watch(&self, _config: &serde_json::Value) -> bool {
        false
    }

    /// Whether this adapter, under `config`, drains [`AdapterCtx::interrupts`].
    fn interrupts(&self, _config: &serde_json::Value) -> bool {
        false
    }
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

    async fn ack_marks(&mut self, _marks: Vec<(String, u64)>) -> CommandOutcome {
        unsupported("ack_marks")
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
}

/// Route one command to its driver method and report its outcome.
///
/// The only `match` over [`AdapterCommand`] the adapters have; it has no
/// wildcard arm, so a new variant needs a driver method.
pub async fn dispatch_command<D: SessionDriver + ?Sized>(
    driver: &mut D,
    events: &mpsc::Sender<AdapterEvent>,
    cmd: AdapterCommand,
) {
    let command_id = cmd.command_id();
    let outcome = match cmd {
        AdapterCommand::ResumeMarks { marks } => driver.resume_marks(marks).await,
        AdapterCommand::AckMarks { marks } => driver.ack_marks(marks).await,
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
        // Routed onto the adapter's `pty_watch` channel by the supervisor.
        AdapterCommand::WatchPty { local_id, watch } => {
            tracing::warn!(%local_id, watch, "watch_pty reached the command loop");
            Ok(Handled::Done)
        }
    };
    report_outcome(driver.adapter_id(), events, command_id, outcome).await;
}

async fn report_outcome(
    adapter: &'static str,
    events: &mpsc::Sender<AdapterEvent>,
    command_id: Option<Uuid>,
    outcome: CommandOutcome,
) {
    report_outcomes(adapter, events, command_id.as_slice(), outcome).await;
}

async fn report_outcomes(
    adapter: &'static str,
    events: &mpsc::Sender<AdapterEvent>,
    command_ids: &[Uuid],
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
    for &command_id in command_ids {
        let event =
            AdapterEvent::CommandResult { command_id, ok: error.is_none(), error: error.clone() };
        if events.send(event).await.is_err() {
            tracing::debug!(adapter, %command_id, "command result dropped: event channel closed");
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interrupts_coalesce_per_session_in_arrival_order() {
        let queue = InterruptQueue::default();
        let (a, b, c) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        queue.push("s1", Some(a));
        queue.push("s2", None);
        queue.push("s1", Some(b));
        queue.push("s2", Some(c));
        assert_eq!(
            queue.drain(),
            vec![
                PendingInterrupt { local_id: "s1".to_owned(), command_ids: vec![a, b] },
                PendingInterrupt { local_id: "s2".to_owned(), command_ids: vec![c] },
            ],
        );
        assert!(queue.drain().is_empty());
    }

    #[test]
    fn spamming_stop_caps_the_kept_ids() {
        let queue = InterruptQueue::default();
        for _ in 0..(MAX_COALESCED_IDS * 4) {
            queue.push("s1", Some(Uuid::new_v4()));
        }
        let pending = queue.drain();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].command_ids.len(), MAX_COALESCED_IDS);
    }

    #[tokio::test]
    async fn next_wakes_on_a_push_made_before_it_waits() {
        let queue = InterruptQueue::default();
        queue.push("s1", None);
        let got = tokio::time::timeout(Duration::from_secs(1), queue.next()).await.unwrap();
        assert_eq!(got.local_id, "s1");
        let waiter = tokio::spawn({
            let queue = queue.clone();
            async move { queue.next().await }
        });
        tokio::task::yield_now().await;
        queue.push("s2", None);
        let got = tokio::time::timeout(Duration::from_secs(1), waiter).await.unwrap().unwrap();
        assert_eq!(got.local_id, "s2");
    }

    struct Blocked {
        release: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl Interrupter for Blocked {
        async fn interrupt(&self, local_id: &str, _command_ids: &[Uuid]) -> CommandOutcome {
            if local_id == "stuck" {
                self.release.notified().await;
            }
            Ok(Handled::Done)
        }
    }

    #[tokio::test]
    async fn a_slow_interrupt_does_not_hold_up_another_session() {
        let queue = InterruptQueue::default();
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let release = Arc::new(tokio::sync::Notify::new());
        let pump = spawn_interrupt_pump(
            "test",
            queue.clone(),
            Arc::new(Blocked { release: Arc::clone(&release) }),
            events_tx,
            shutdown.clone(),
        );
        let (stuck, fast) = (Uuid::new_v4(), Uuid::new_v4());
        queue.push("stuck", Some(stuck));
        queue.push("fast", Some(fast));
        let first = tokio::time::timeout(Duration::from_secs(1), events_rx.recv())
            .await
            .expect("the second session's interrupt must not wait for the first")
            .unwrap();
        assert!(matches!(
            first,
            AdapterEvent::CommandResult { command_id, ok: true, .. } if command_id == fast
        ));
        release.notify_one();
        let second =
            tokio::time::timeout(Duration::from_secs(1), events_rx.recv()).await.unwrap().unwrap();
        assert!(matches!(
            second,
            AdapterEvent::CommandResult { command_id, ok: true, .. } if command_id == stuck
        ));
        shutdown.cancel();
        pump.await.unwrap();
    }

    struct Failing;

    #[async_trait::async_trait]
    impl Interrupter for Failing {
        async fn interrupt(&self, _local_id: &str, _command_ids: &[Uuid]) -> CommandOutcome {
            Err(anyhow::anyhow!("no live session"))
        }
    }

    #[tokio::test]
    async fn every_coalesced_id_hears_the_outcome() {
        let queue = InterruptQueue::default();
        let (events_tx, mut events_rx) = mpsc::channel(8);
        let shutdown = CancellationToken::new();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        queue.push("s1", [a, b]);
        let pump =
            spawn_interrupt_pump("test", queue, Arc::new(Failing), events_tx, shutdown.clone());
        let mut answered = Vec::new();
        for _ in 0..2 {
            match tokio::time::timeout(Duration::from_secs(1), events_rx.recv()).await.unwrap() {
                Some(AdapterEvent::CommandResult { command_id, ok: false, error }) => {
                    assert_eq!(error.as_deref(), Some("no live session"));
                    answered.push(command_id);
                }
                other => panic!("expected a failed CommandResult, got {other:?}"),
            }
        }
        assert_eq!(answered, vec![a, b]);
        shutdown.cancel();
        pump.await.unwrap();
    }
}
