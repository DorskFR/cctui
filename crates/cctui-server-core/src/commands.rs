//! Launch acknowledgment, failure metadata and bounded pending-command tracking.

use std::sync::Arc;
use std::time::{Duration, Instant};

use dashmap::DashMap;
use uuid::Uuid;

/// A failed spawn's `sessions` row: the daemon never registers a session that
/// failed to start, so its identity survives only here.
#[derive(Debug, Clone)]
pub struct FailedSpawnRow {
    pub session_id: String,
    pub machine_id: Uuid,
    pub user_id: Uuid,
    pub adapter_id: String,
    pub working_dir: String,
    pub name: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PendingCommand {
    pub session_id: Option<String>,
    /// Consumed only when the spawn fails.
    pub spawn: Option<FailedSpawnRow>,
    pub at: Instant,
}

pub const PENDING_COMMAND_TTL: Duration = Duration::from_mins(10);

/// A daemon's verdict on a command, handed to a request still waiting for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandVerdict {
    pub ok: bool,
    pub error: Option<String>,
}

pub type CommandWaiters = Arc<DashMap<Uuid, tokio::sync::oneshot::Sender<CommandVerdict>>>;

/// How long a spawn request holds its reply for an early daemon refusal.
///
/// The daemon refuses on receipt (unknown adapter, full command queue), so a
/// refusal lands within milliseconds; a slower verdict reaches the client
/// over the WS as before.
pub const EARLY_VERDICT_WAIT: Duration = Duration::from_secs(1);

/// Register a waiter for `command_id`. Call before the command is sent, so a
/// verdict can never arrive ahead of it.
#[must_use]
pub fn wait_for_verdict(
    waiters: &CommandWaiters,
    command_id: Uuid,
) -> tokio::sync::oneshot::Receiver<CommandVerdict> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    waiters.insert(command_id, tx);
    rx
}

/// Hand `verdict` to the request waiting on `command_id`, if any is.
pub fn resolve_verdict(waiters: &CommandWaiters, command_id: Uuid, verdict: CommandVerdict) {
    if let Some((_, tx)) = waiters.remove(&command_id) {
        // The request may have stopped waiting a moment ago: nothing to do.
        let _ = tx.send(verdict);
    }
}

struct VerdictRegistration<'a> {
    waiters: &'a CommandWaiters,
    command_id: Uuid,
}

impl Drop for VerdictRegistration<'_> {
    fn drop(&mut self) {
        self.waiters.remove(&self.command_id);
    }
}

/// Wait up to `wait` for the daemon's verdict on `command_id` and return the
/// refusal's detail, or `None` when the command was accepted or no verdict
/// came in time. Always unregisters the waiter.
pub async fn await_early_verdict(
    waiters: &CommandWaiters,
    command_id: Uuid,
    rx: tokio::sync::oneshot::Receiver<CommandVerdict>,
    wait: Duration,
) -> Option<String> {
    let registration = VerdictRegistration { waiters, command_id };
    let verdict = tokio::time::timeout(wait, rx).await;
    drop(registration);
    match verdict {
        Ok(Ok(CommandVerdict { ok: false, error })) => {
            Some(error.unwrap_or_else(|| "spawn failed".to_owned()))
        }
        _ => None,
    }
}

pub fn track_command(
    map: &DashMap<Uuid, PendingCommand>,
    command_id: Uuid,
    session_id: Option<String>,
    spawn: Option<FailedSpawnRow>,
) {
    let now = Instant::now();
    map.retain(|_, c| now.duration_since(c.at) < PENDING_COMMAND_TTL);
    map.insert(command_id, PendingCommand { session_id, spawn, at: now });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waiters() -> CommandWaiters {
        Arc::new(DashMap::new())
    }

    /// The incident of 11/10/2026: the daemon turned the spawn away 33 ms after
    /// it was sent, and the route had already answered 202.
    #[tokio::test(start_paused = true)]
    async fn an_early_refusal_reaches_the_waiting_request() {
        let waiters = waiters();
        let command_id = Uuid::new_v4();
        let rx = wait_for_verdict(&waiters, command_id);
        let daemon = {
            let waiters = waiters.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(33)).await;
                resolve_verdict(
                    &waiters,
                    command_id,
                    CommandVerdict {
                        ok: false,
                        error: Some("adapter claude-code command queue is full".into()),
                    },
                );
            })
        };
        let started = tokio::time::Instant::now();
        let refusal = await_early_verdict(&waiters, command_id, rx, EARLY_VERDICT_WAIT).await;
        assert_eq!(refusal.as_deref(), Some("adapter claude-code command queue is full"));
        assert!(
            started.elapsed() < EARLY_VERDICT_WAIT,
            "answered on the refusal, not the deadline"
        );
        assert!(waiters.is_empty());
        daemon.await.unwrap();
    }

    #[tokio::test(start_paused = true)]
    async fn no_verdict_in_time_lets_the_spawn_through() {
        let waiters = waiters();
        let command_id = Uuid::new_v4();
        let rx = wait_for_verdict(&waiters, command_id);
        assert_eq!(await_early_verdict(&waiters, command_id, rx, EARLY_VERDICT_WAIT).await, None);
        assert!(waiters.is_empty(), "the waiter is gone once the request stops waiting");
        // A verdict arriving afterwards finds nobody and is a no-op.
        resolve_verdict(&waiters, command_id, CommandVerdict { ok: false, error: None });
    }

    #[tokio::test(start_paused = true)]
    async fn an_accepted_spawn_is_not_a_refusal() {
        let waiters = waiters();
        let command_id = Uuid::new_v4();
        let rx = wait_for_verdict(&waiters, command_id);
        resolve_verdict(&waiters, command_id, CommandVerdict { ok: true, error: None });
        assert_eq!(await_early_verdict(&waiters, command_id, rx, EARLY_VERDICT_WAIT).await, None);
    }

    #[tokio::test]
    async fn a_refusal_without_detail_still_says_why() {
        let waiters = waiters();
        let command_id = Uuid::new_v4();
        let rx = wait_for_verdict(&waiters, command_id);
        resolve_verdict(&waiters, command_id, CommandVerdict { ok: false, error: None });
        assert_eq!(
            await_early_verdict(&waiters, command_id, rx, EARLY_VERDICT_WAIT).await.as_deref(),
            Some("spawn failed")
        );
    }
}
