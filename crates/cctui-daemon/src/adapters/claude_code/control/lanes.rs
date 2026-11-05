//! One off-loop op in flight per session; later commands for it wait their turn.

use std::collections::{HashMap, VecDeque};
use std::future::Future;

use cctui_proto::adapter::AdapterCommand;
use tokio::sync::mpsc;
use tracing::Instrument as _;

use super::{AdapterEvent, Driver};

pub(super) struct LaneDone {
    local_id: String,
    undelivered: Vec<String>,
}

pub(super) struct LaneOutcome {
    pub(super) result: anyhow::Result<()>,
    pub(super) undelivered: Vec<String>,
}

impl From<anyhow::Result<()>> for LaneOutcome {
    fn from(result: anyhow::Result<()>) -> Self {
        Self { result, undelivered: Vec::new() }
    }
}

pub(super) struct Lanes {
    /// Keyed by sessions with an op in flight.
    busy: HashMap<String, VecDeque<AdapterCommand>>,
    done_tx: mpsc::UnboundedSender<LaneDone>,
    pub(super) done_rx: mpsc::UnboundedReceiver<LaneDone>,
}

impl Default for Lanes {
    fn default() -> Self {
        let (done_tx, done_rx) = mpsc::unbounded_channel();
        Self { busy: HashMap::new(), done_tx, done_rx }
    }
}

impl Lanes {
    pub(super) fn is_busy(&self, local_id: &str) -> bool {
        self.busy.contains_key(local_id)
    }

    #[cfg(test)]
    pub(super) fn queued(&self, local_id: &str) -> usize {
        self.busy.get(local_id).map_or(0, VecDeque::len)
    }

    fn hold(&mut self, cmd: AdapterCommand) -> Option<AdapterCommand> {
        let key = ordered_session(&cmd).map(str::to_owned);
        match key.and_then(|id| self.busy.get_mut(&id)) {
            Some(queue) => {
                queue.push_back(cmd);
                None
            }
            None => Some(cmd),
        }
    }
}

/// Commands that act on one session's worker and must not overtake each
/// other. Interrupt is left out on purpose: Stop never waits for a reply.
fn ordered_session(cmd: &AdapterCommand) -> Option<&str> {
    match cmd {
        AdapterCommand::Reply { local_id, .. }
        | AdapterCommand::SendMessage { local_id, .. }
        | AdapterCommand::Remove { local_id, .. }
        | AdapterCommand::Kill { local_id, .. }
        | AdapterCommand::Resume { local_id, .. }
        | AdapterCommand::PermissionResponse { local_id, .. }
        | AdapterCommand::Rename { local_id, .. }
        | AdapterCommand::SetModel { local_id, .. } => Some(local_id),
        _ => None,
    }
}

/// Frees the lane on drop, so a panicking op cannot wedge its session.
struct LaneTicket {
    local_id: String,
    undelivered: Vec<String>,
    done: mpsc::UnboundedSender<LaneDone>,
}

impl Drop for LaneTicket {
    fn drop(&mut self) {
        let _ = self.done.send(LaneDone {
            local_id: std::mem::take(&mut self.local_id),
            undelivered: std::mem::take(&mut self.undelivered),
        });
    }
}

impl Driver {
    pub(super) async fn route_command(
        &mut self,
        events: &mpsc::Sender<AdapterEvent>,
        cmd: AdapterCommand,
    ) {
        if let Some(cmd) = self.lanes.hold(cmd) {
            crate::adapter_runtime::dispatch_command(self, events, cmd).await;
        }
    }

    /// The caller answers `Handled::Deferred`: this reports `command_id`.
    pub(super) fn run_in_lane<F>(&mut self, local_id: &str, command_id: Option<uuid::Uuid>, op: F)
    where
        F: Future<Output = LaneOutcome> + Send + 'static,
    {
        self.lanes.busy.entry(local_id.to_owned()).or_default();
        let mut ticket = LaneTicket {
            local_id: local_id.to_owned(),
            undelivered: Vec::new(),
            done: self.lanes.done_tx.clone(),
        };
        let events = self.events.clone();
        tokio::spawn(
            async move {
                let LaneOutcome { result, undelivered } = op.await;
                if let Err(err) = &result {
                    tracing::warn!(local_id = %ticket.local_id, %err, "command dispatch failed");
                }
                ticket.undelivered = undelivered;
                Self::report_command(&events, command_id, result).await;
                drop(ticket);
            }
            .instrument(tracing::Span::current()),
        );
    }

    pub(super) async fn finish_lane(
        &mut self,
        events: &mpsc::Sender<AdapterEvent>,
        done: LaneDone,
    ) {
        let LaneDone { local_id, undelivered } = done;
        if !undelivered.is_empty() {
            let queue = self.deferred_peer.entry(local_id.clone()).or_default();
            for text in undelivered.into_iter().rev() {
                queue.push_front(text);
            }
        }
        let mut queued = self.lanes.busy.remove(&local_id).unwrap_or_default();
        while let Some(cmd) = queued.pop_front() {
            if let Some(waiting) = self.lanes.busy.get_mut(&local_id) {
                waiting.push_back(cmd);
                waiting.extend(queued);
                return;
            }
            crate::adapter_runtime::dispatch_command(self, events, cmd).await;
        }
    }
}

pub(super) async fn deliver_in_order(jobs: Vec<(super::reply::ReplyJob, String)>) -> LaneOutcome {
    let mut jobs = jobs.into_iter();
    while let Some((job, text)) = jobs.next() {
        match Box::pin(job.deliver()).await {
            Ok(()) => {}
            Err(err) if err.is::<super::reply::AskOpened>() => {
                let undelivered = std::iter::once(text).chain(jobs.map(|(_, t)| t)).collect();
                return LaneOutcome { result: Ok(()), undelivered };
            }
            Err(err) => tracing::warn!(%err, "peer turn not delivered"),
        }
    }
    Ok(()).into()
}
