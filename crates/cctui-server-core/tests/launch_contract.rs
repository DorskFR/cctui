//! Prepared launches use the real delivery bus without workspace services.
use std::sync::Arc;
use std::time::Duration;

use cctui_proto::adapter::AdapterCommand;
use cctui_proto::ws::DaemonFrameDown;
use cctui_server_core::bus::{Bus, BusError, NoopTransport};
use cctui_server_core::commands::{
    CommandVerdict, CommandWaiters, FailedSpawnRow, PendingCommand, resolve_verdict,
};
use cctui_server_core::launch::{LaunchError, MachineSpawn, launch_machine};
use dashmap::DashMap;
use tokio::sync::mpsc;
use uuid::Uuid;

struct Host {
    bus: Bus,
    pending: DashMap<Uuid, PendingCommand>,
    waiters: CommandWaiters,
}

impl Host {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            bus: Bus::new(Box::new(NoopTransport)),
            pending: DashMap::new(),
            waiters: Arc::new(DashMap::new()),
        })
    }

    fn spawn(machine_id: Uuid) -> MachineSpawn {
        let token_key = Uuid::new_v4();
        MachineSpawn {
            machine_id,
            command_id: Uuid::new_v4(),
            token_key,
            spec: serde_json::from_value(serde_json::json!({
                "adapter_id": "codex", "working_dir": "/tmp", "prompt": "review changes",
                "env": {"TEST_SECRET": "fixture-secret"}
            }))
            .unwrap(),
            failed: FailedSpawnRow {
                session_id: token_key.to_string(),
                machine_id,
                user_id: Uuid::new_v4(),
                adapter_id: "codex".into(),
                working_dir: "/tmp".into(),
                name: None,
                model: None,
                effort: None,
            },
        }
    }

    fn start(
        self: &Arc<Self>,
        spawn: MachineSpawn,
    ) -> tokio::task::JoinHandle<Result<(), LaunchError>> {
        let host = self.clone();
        tokio::spawn(
            async move { launch_machine(&host.bus, &host.pending, &host.waiters, spawn).await },
        )
    }
}

#[tokio::test]
async fn an_offline_target_leaves_no_command_or_waiter() {
    let host = Host::new();
    let result = host.start(Host::spawn(Uuid::new_v4())).await.unwrap();
    assert!(matches!(result, Err(LaunchError::Delivery(BusError::NoDaemon(_)))));
    assert!(host.pending.is_empty());
    assert!(host.waiters.is_empty());
}

#[tokio::test]
async fn immediate_refusal_keeps_failure_metadata_and_returns_the_reason() {
    let host = Host::new();
    let machine = Uuid::new_v4();
    let (tx, mut rx) = mpsc::channel(1);
    host.bus.register_daemon(machine, Uuid::new_v4(), tx);
    let spawn = Host::spawn(machine);
    let (command_id, token_key) = (spawn.command_id, spawn.token_key);
    let request = host.start(spawn);
    let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap();
    let DaemonFrameDown::Command { adapter_id, command } = frame else {
        panic!("expected command")
    };
    let AdapterCommand::Spawn { command_id: sent, session_id, spec } = *command else {
        panic!("expected spawn")
    };
    assert_eq!(adapter_id, "codex");
    assert_eq!(sent, Some(command_id));
    assert_eq!(session_id, Some(token_key));
    assert_eq!(spec.env["TEST_SECRET"], "fixture-secret");
    assert!(host.pending.contains_key(&command_id));
    resolve_verdict(
        &host.waiters,
        command_id,
        CommandVerdict { ok: false, error: Some("queue full".into()) },
    );
    let result = tokio::time::timeout(Duration::from_secs(2), request).await.unwrap().unwrap();
    assert!(matches!(result, Err(LaunchError::Refused(reason)) if reason == "queue full"));
    assert!(host.waiters.is_empty());
    assert_eq!(
        host.pending.get(&command_id).unwrap().spawn.as_ref().unwrap().session_id,
        token_key.to_string()
    );
}

#[tokio::test(start_paused = true)]
async fn a_slow_verdict_preserves_the_existing_acceptance_deadline() {
    let host = Host::new();
    let machine = Uuid::new_v4();
    let (tx, mut rx) = mpsc::channel(1);
    host.bus.register_daemon(machine, Uuid::new_v4(), tx);
    let spawn = Host::spawn(machine);
    let command_id = spawn.command_id;
    let started = tokio::time::Instant::now();
    let request = host.start(spawn);
    rx.recv().await.unwrap();
    request.await.unwrap().unwrap();
    assert_eq!(started.elapsed(), cctui_server_core::commands::EARLY_VERDICT_WAIT);
    assert!(host.waiters.is_empty());
    assert!(host.pending.contains_key(&command_id), "later daemon results still need metadata");
}

#[tokio::test]
async fn canceling_while_the_daemon_send_is_blocked_unregisters_the_waiter() {
    let host = Host::new();
    let machine = Uuid::new_v4();
    let (tx, _rx) = mpsc::channel(1);
    host.bus.register_daemon(machine, Uuid::new_v4(), tx);
    host.bus
        .command_daemon(
            machine,
            DaemonFrameDown::Reconcile {
                adapters: vec![],
                secret_scrub: cctui_proto::ws::SecretScrubConfig::default(),
            },
        )
        .await
        .unwrap();
    let spawn = Host::spawn(machine);
    let command_id = spawn.command_id;
    let request = host.start(spawn);
    tokio::time::timeout(Duration::from_secs(2), async {
        while !host.waiters.contains_key(&command_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("launch must reach the blocked send");
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert!(host.waiters.is_empty());
}
