//! Delivery and early acknowledgment for a prepared, authorized machine spawn.

use cctui_proto::adapter::{AdapterCommand, SessionSpec};
use cctui_proto::models::MachineLiveness;
use cctui_proto::ws::DaemonFrameDown;
use chrono::{DateTime, Utc};
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
    /// Deliver on the connection that announced this session, not the
    /// machine's newest one: dispatched workers share one machine identity.
    pub route_session: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error("could not persist machine launch: {0}")]
    Storage(#[from] sqlx::Error),
    #[error(transparent)]
    Delivery(#[from] BusError),
    #[error("the daemon refused the spawn: {0}")]
    Refused(String),
    #[error("hook policy: {0}")]
    Policy(String),
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
    let MachineSpawn { machine_id, command_id, token_key, spec, failed, route_session } = spawn;
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
    let delivered = match route_session.as_deref() {
        Some(session) => bus.command_daemon_for_session(machine_id, session, frame).await,
        None => bus.command_daemon(machine_id, frame).await,
    };
    if let Err(error) = delivered {
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
    mut spawn: MachineSpawn,
) -> Result<(), LaunchError> {
    let command = spawn.command_id;
    let rules = crate::hook_policy::account_rules(&state.pool, &spawn.failed.session_id).await?;
    let carried =
        crate::hook_policy::attach(&mut spawn.spec, rules).map_err(LaunchError::Policy)?;
    crate::machine_launches::prepare(&state.pool, command, actor, &spawn.failed).await?;
    crate::hook_policy::record(&state.pool, command, &carried).await?;
    let result =
        launch_machine(&state.bus, &state.pending_commands, &state.command_waiters, spawn).await;
    match &result {
        Ok(()) => crate::machine_launches::delivered(&state.pool, command).await?,
        Err(LaunchError::Delivery(error)) => {
            crate::machine_launches::delivery_failed(&state.pool, command, &error.to_string())
                .await?;
        }
        Err(LaunchError::Refused(_) | LaunchError::Storage(_) | LaunchError::Policy(_)) => {}
    }
    result
}

/// What a launch tells the user when the daemon's machine is offline.
pub const DAEMON_OFFLINE: &str = "daemon for that machine is offline — start `cctui-daemon` first";

/// What a launch tells the user when the machine is online but no replica
/// reached its daemon connection.
pub const DAEMON_REPLICA_UNREACHABLE: &str =
    "could not reach the daemon's replica — the machine is online, try again";

/// The no-daemon message for a machine in `liveness`.
#[must_use]
pub const fn no_daemon_message(liveness: Option<MachineLiveness>) -> &'static str {
    match liveness {
        Some(MachineLiveness::Online) => DAEMON_REPLICA_UNREACHABLE,
        Some(MachineLiveness::Stale | MachineLiveness::Offline) | None => DAEMON_OFFLINE,
    }
}

/// [`no_daemon_message`] for `machine`'s heartbeat as stored.
///
/// A heartbeat inside the online window means a live daemon some replica
/// holds, so the miss is cross-replica routing, not the daemon being down.
pub async fn no_daemon_detail(pool: &sqlx::PgPool, machine: Uuid) -> &'static str {
    let last_seen =
        sqlx::query_scalar::<_, DateTime<Utc>>("SELECT last_seen_at FROM machines WHERE id = $1")
            .bind(machine)
            .fetch_optional(pool)
            .await
            .unwrap_or_else(|err| {
                tracing::warn!(%err, %machine, "machine liveness read failed");
                None
            });
    no_daemon_message(last_seen.map(crate::targets::derive_liveness))
}

/// What a launch tells the user when its command never reached the daemon.
pub const DAEMON_DISCONNECTED: &str = "daemon disconnected mid-dispatch";

/// The detail for a delivery to `machine` that failed with `error`.
pub async fn delivery_detail(pool: &sqlx::PgPool, machine: Uuid, error: &BusError) -> &'static str {
    match error {
        BusError::NoDaemon(_) => no_daemon_detail(pool, machine).await,
        _ => DAEMON_DISCONNECTED,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_online_machine_is_never_called_offline() {
        assert_eq!(no_daemon_message(Some(MachineLiveness::Online)), DAEMON_REPLICA_UNREACHABLE);
        assert!(!DAEMON_REPLICA_UNREACHABLE.contains("offline"));
        assert!(DAEMON_REPLICA_UNREACHABLE.contains("could not reach the daemon's replica"));
        for tier in [Some(MachineLiveness::Stale), Some(MachineLiveness::Offline), None] {
            assert_eq!(no_daemon_message(tier), DAEMON_OFFLINE);
        }
    }

    #[tokio::test]
    async fn the_stored_heartbeat_picks_the_message() {
        let Some(url) = crate::test_support::db_url("no_daemon_detail") else { return };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let user = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(user)
            .bind(format!("no-daemon-{user}"))
            .bind(format!("kh-{user}"))
            .execute(&pool)
            .await
            .unwrap();
        let seed = async |age: chrono::Duration| {
            let machine = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash, last_seen_at) \
                 VALUES ($1, $2, $3, $4, $5)",
            )
            .bind(machine)
            .bind(user)
            .bind(format!("m-{machine}"))
            .bind(format!("mk-{machine}"))
            .bind(Utc::now() - age)
            .execute(&pool)
            .await
            .unwrap();
            machine
        };
        let online = seed(chrono::Duration::seconds(10)).await;
        let gone = seed(chrono::Duration::hours(2)).await;

        assert_eq!(no_daemon_detail(&pool, online).await, DAEMON_REPLICA_UNREACHABLE);
        assert_eq!(no_daemon_detail(&pool, gone).await, DAEMON_OFFLINE);
        assert_eq!(no_daemon_detail(&pool, Uuid::new_v4()).await, DAEMON_OFFLINE);
    }
}
