//! Low-priority lane for job removals.
//!
//! An adapter drains its command channel serially, and a `Remove` is slow
//! (stop the worker, wait for it to exit, `claude rm`). A daemon coming back
//! from a freeze gets a burst of them at once: the purge of archived jobs plus
//! the archives the server swept meanwhile. Fed straight into the 64-deep
//! command channel they fill it, and the next `Spawn` is rejected with
//! "command queue is full" (11/10/2026: 25 removals, 7 commands rejected,
//! one of them a phone spawn).
//!
//! Removals therefore wait here and reach the adapter one at a time, only when
//! its command channel is empty. Whatever the server sends meanwhile (a spawn,
//! a prompt, a permission answer) finds the channel free and runs after at most
//! the one removal already handed over.

use std::time::Duration;

use cctui_proto::adapter::AdapterCommand;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// Removals the lane holds before refusing more. A backlog of weeks of
/// archives is a few hundred; the purge task waits for room rather than
/// failing, so only a burst of server archives can ever hit this.
pub const MAINTENANCE_BUFFER: usize = 1024;

/// How often the feeder re-checks whether the adapter's channel has drained.
const IDLE_POLL: Duration = Duration::from_millis(100);

/// Whether `command` belongs on the maintenance lane rather than the
/// adapter's command channel.
#[must_use]
pub const fn is_maintenance(command: &AdapterCommand) -> bool {
    matches!(command, AdapterCommand::Remove { .. })
}

/// Sender side of an adapter's maintenance lane. Cheap to clone.
#[derive(Clone, Debug)]
pub struct MaintenanceLane {
    tx: mpsc::Sender<AdapterCommand>,
}

impl MaintenanceLane {
    /// Start the feeder that moves queued removals into `commands_tx` one at a
    /// time, each only once the channel is empty. The feeder stops with
    /// `shutdown` or when the adapter's channel closes.
    #[must_use]
    pub fn spawn(
        commands_tx: mpsc::Sender<AdapterCommand>,
        shutdown: CancellationToken,
    ) -> (Self, JoinHandle<()>) {
        Self::with_capacity(commands_tx, shutdown, MAINTENANCE_BUFFER)
    }

    fn with_capacity(
        commands_tx: mpsc::Sender<AdapterCommand>,
        shutdown: CancellationToken,
        capacity: usize,
    ) -> (Self, JoinHandle<()>) {
        let (tx, rx) = mpsc::channel(capacity);
        let feeder = tokio::spawn(feed(rx, commands_tx, shutdown));
        (Self { tx }, feeder)
    }

    /// Queue without waiting; for the transport loop, which must never block.
    ///
    /// # Errors
    /// The command comes back when the lane is full or its feeder has stopped.
    pub fn try_push(
        &self,
        command: AdapterCommand,
    ) -> Result<(), Box<mpsc::error::TrySendError<AdapterCommand>>> {
        self.tx.try_send(command).map_err(Box::new)
    }

    /// Queue, waiting for room; for the background purge.
    ///
    /// # Errors
    /// The command comes back when the feeder has stopped.
    pub async fn push(
        &self,
        command: AdapterCommand,
    ) -> Result<(), mpsc::error::SendError<AdapterCommand>> {
        self.tx.send(command).await
    }
}

async fn feed(
    mut rx: mpsc::Receiver<AdapterCommand>,
    commands_tx: mpsc::Sender<AdapterCommand>,
    shutdown: CancellationToken,
) {
    loop {
        let command = tokio::select! {
            () = shutdown.cancelled() => return,
            next = rx.recv() => match next {
                Some(command) => command,
                None => return,
            },
        };
        // Hand over only into an empty channel, so a removal never sits in
        // front of more than the one the adapter may be running already.
        while commands_tx.capacity() < commands_tx.max_capacity() {
            if commands_tx.is_closed() {
                return;
            }
            tokio::select! {
                () = shutdown.cancelled() => return,
                () = tokio::time::sleep(IDLE_POLL) => {}
            }
        }
        // The channel was empty a moment ago: at worst a few priority commands
        // slipped in since, so this send does not wait in practice.
        tokio::select! {
            () = shutdown.cancelled() => return,
            sent = commands_tx.send(command) => if sent.is_err() { return },
        }
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::adapter::RemoveInitiator;

    use super::*;

    fn remove(n: usize) -> AdapterCommand {
        AdapterCommand::Remove {
            local_id: format!("sess-{n}"),
            command_id: None,
            initiator: RemoveInitiator::Automatic,
        }
    }

    fn priority(n: usize) -> AdapterCommand {
        AdapterCommand::AckMarks { marks: vec![(format!("prio-{n}"), 0)] }
    }

    #[test]
    fn only_removals_go_to_the_maintenance_lane() {
        assert!(is_maintenance(&remove(0)));
        assert!(!is_maintenance(&priority(0)));
    }

    /// The regression of 11/10/2026: a backlog of removals larger than the
    /// adapter's channel must leave that channel free for whatever the server
    /// sends next, however long the adapter takes to get through them.
    #[tokio::test]
    async fn a_removal_backlog_never_fills_the_adapter_channel() {
        let (commands_tx, mut commands_rx) = mpsc::channel(4);
        let shutdown = CancellationToken::new();
        let (lane, feeder) = MaintenanceLane::spawn(commands_tx.clone(), shutdown.clone());
        for n in 0..50 {
            lane.try_push(remove(n)).expect("lane has room");
        }
        // The adapter is busy and drains nothing: give the feeder every chance
        // to over-fill the channel.
        tokio::time::sleep(IDLE_POLL * 5).await;
        assert_eq!(
            commands_tx.max_capacity() - commands_tx.capacity(),
            1,
            "exactly one removal handed over while the adapter is busy"
        );
        // A spawn (any priority command) still fits, several times over.
        for n in 0..3 {
            commands_tx.try_send(priority(n)).expect("priority command accepted");
        }
        // Drain: the removal already handed over, then the priority commands,
        // then the rest of the backlog in order.
        let mut seen = Vec::new();
        while seen.len() < 53 {
            let command = tokio::time::timeout(Duration::from_secs(5), commands_rx.recv())
                .await
                .expect("backlog keeps flowing")
                .expect("channel open");
            seen.push(match command {
                AdapterCommand::Remove { local_id, .. } => local_id,
                AdapterCommand::AckMarks { marks } => marks[0].0.clone(),
                other => panic!("unexpected {other:?}"),
            });
        }
        assert_eq!(&seen[..4], ["sess-0", "prio-0", "prio-1", "prio-2"]);
        let rest: Vec<String> = (1..50).map(|n| format!("sess-{n}")).collect();
        assert_eq!(&seen[4..], rest.as_slice());
        shutdown.cancel();
        feeder.await.unwrap();
    }

    #[tokio::test]
    async fn a_full_lane_refuses_without_blocking() {
        let (commands_tx, _commands_rx) = mpsc::channel(1);
        // Occupy the adapter channel so the feeder holds on to what it takes.
        commands_tx.try_send(priority(0)).unwrap();
        let shutdown = CancellationToken::new();
        let (lane, _feeder) = MaintenanceLane::with_capacity(commands_tx, shutdown.clone(), 2);
        let mut refused = 0;
        for n in 0..10 {
            if lane.try_push(remove(n)).is_err() {
                refused += 1;
            }
        }
        // Two in the lane, at most one in the feeder's hands.
        assert!(refused >= 7, "refused {refused}");
        shutdown.cancel();
    }

    #[tokio::test]
    async fn the_feeder_stops_with_the_adapter() {
        let (commands_tx, commands_rx) = mpsc::channel(1);
        let shutdown = CancellationToken::new();
        let (lane, feeder) = MaintenanceLane::spawn(commands_tx, shutdown.clone());
        drop(commands_rx);
        lane.try_push(remove(0)).unwrap();
        tokio::time::timeout(Duration::from_secs(2), feeder)
            .await
            .expect("feeder exits once the adapter channel is closed")
            .unwrap();
        assert!(lane.try_push(remove(1)).is_err());
    }
}
