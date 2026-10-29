//! A headless host gets the same daemon-verdict lifecycle as the full server.
use std::sync::Arc;
use std::time::{Duration, Instant};

use cctui_server_core::commands::{
    CommandWaiters, EARLY_VERDICT_WAIT, PENDING_COMMAND_TTL, PendingCommand, await_early_verdict,
    track_command, wait_for_verdict,
};
use dashmap::DashMap;
use uuid::Uuid;

#[tokio::test(start_paused = true)]
async fn cancelling_a_request_unregisters_its_daemon_verdict_waiter() {
    let waiters: CommandWaiters = Arc::new(DashMap::new());
    let command = Uuid::new_v4();
    let receiver = wait_for_verdict(&waiters, command);
    let (started, ready) = tokio::sync::oneshot::channel();
    let request = {
        let waiters = waiters.clone();
        tokio::spawn(async move {
            started.send(()).unwrap();
            await_early_verdict(&waiters, command, receiver, EARLY_VERDICT_WAIT).await
        })
    };
    ready.await.unwrap();
    assert!(waiters.contains_key(&command));
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert!(waiters.is_empty(), "a disconnected caller must not leave a waiter behind");
}

#[test]
fn tracking_a_command_prunes_expired_metadata_and_keeps_live_commands() {
    let commands = DashMap::new();
    let old = Uuid::new_v4();
    let live = Uuid::new_v4();
    commands.insert(
        old,
        PendingCommand {
            session_id: Some("expired".into()),
            spawn: None,
            at: Instant::now().checked_sub(PENDING_COMMAND_TTL + Duration::from_secs(1)).unwrap(),
        },
    );
    track_command(&commands, live, Some("running".into()), None);
    let next = Uuid::new_v4();
    track_command(&commands, next, None, None);
    assert!(!commands.contains_key(&old));
    assert_eq!(commands.get(&live).unwrap().session_id.as_deref(), Some("running"));
    assert!(commands.contains_key(&next));
}
