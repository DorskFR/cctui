//! Delivery and early acknowledgment for a prepared, authorized machine spawn.

use cctui_proto::adapter::{AdapterCommand, SessionSpec};
use cctui_proto::ws::DaemonFrameDown;
use dashmap::DashMap;
use uuid::Uuid;

use crate::bus::{Bus, BusError};
use crate::commands::{
    CommandWaiters, EARLY_VERDICT_WAIT, FailedSpawnRow, PendingCommand, await_early_verdict,
    track_command, wait_for_verdict,
};

/// The authorized target and prepared worker input, without workspace storage.
pub struct MachineSpawn {
    pub machine_id: Uuid,
    pub command_id: Uuid,
    pub token_key: Uuid,
    pub spec: SessionSpec,
    pub failed: FailedSpawnRow,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("could not persist machine launch: {0}")]
    Storage(#[from] sqlx::Error),
    #[error(transparent)]
    Delivery(#[from] BusError),
    #[error("the daemon refused the spawn: {0}")]
    Refused(String),
}

/// Pre-mint the session identity only for a harness that accepts it.
///
/// Other harnesses echo the command key when announcing their own session ID,
/// allowing the host to rebind temporary credentials and launch intents.
#[must_use]
pub fn spawn_ids(adapter_id: &str, command_id: Uuid) -> (Option<Uuid>, Uuid) {
    let pre_session_id = (adapter_id == "claude-code").then(Uuid::new_v4);
    (pre_session_id, pre_session_id.unwrap_or(command_id))
}

struct LaunchWaiter<'a> {
    waiters: &'a CommandWaiters,
    command_id: Uuid,
}

impl Drop for LaunchWaiter<'_> {
    fn drop(&mut self) {
        self.waiters.remove(&self.command_id);
    }
}

/// Track, deliver and await an immediate refusal using the shared deadline.
///
/// The host authorizes and prepares inputs before calling this service. A send
/// failure removes pending metadata; after delivery it remains for daemon event
/// handling, including when the caller stops waiting. Cancellation abandons the
/// reply waiter, not work already sent to the machine.
pub async fn launch_machine(
    bus: &Bus,
    pending: &DashMap<Uuid, PendingCommand>,
    waiters: &CommandWaiters,
    spawn: MachineSpawn,
) -> Result<(), LaunchError> {
    let MachineSpawn { machine_id, command_id, token_key, spec, failed } = spawn;
    let frame = DaemonFrameDown::Command {
        adapter_id: spec.adapter_id.as_str().to_owned(),
        command: Box::new(AdapterCommand::Spawn {
            spec,
            command_id: Some(command_id),
            session_id: Some(token_key),
        }),
        trace: None,
    };
    let verdict = wait_for_verdict(waiters, command_id);
    let registration = LaunchWaiter { waiters, command_id };
    track_command(pending, command_id, None, Some(failed));
    if let Err(error) = bus.command_daemon(machine_id, frame).await {
        pending.remove(&command_id);
        return Err(LaunchError::Delivery(error));
    }
    let refusal = await_early_verdict(waiters, command_id, verdict, EARLY_VERDICT_WAIT).await;
    drop(registration);
    refusal.map_or(Ok(()), |detail| Err(LaunchError::Refused(detail)))
}

/// Persist a safe launch identity before delivering work. A lost HTTP waiter
/// does not remove the durable record or authorize an automatic resubmission.
pub async fn launch_durable(
    state: &crate::state::CoreState,
    actor: Uuid,
    spawn: MachineSpawn,
) -> Result<(), LaunchError> {
    let command = spawn.command_id;
    crate::machine_launches::prepare(&state.pool, command, actor, &spawn.failed).await?;
    let result =
        launch_machine(&state.bus, &state.pending_commands, &state.command_waiters, spawn).await;
    match &result {
        Ok(()) => crate::machine_launches::delivered(&state.pool, command).await?,
        Err(LaunchError::Delivery(error)) => {
            crate::machine_launches::delivery_failed(&state.pool, command, &error.to_string())
                .await?;
        }
        Err(LaunchError::Refused(_) | LaunchError::Storage(_)) => {}
    }
    result
}
