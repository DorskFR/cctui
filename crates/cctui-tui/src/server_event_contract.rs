//! Every `ServerEvent` the server can send must survive the TUI's decode path.
//!
//! `variant_name` is exhaustive, so adding a variant to the proto stops this
//! file compiling until a sample exists for it; `VARIANT_COUNT` then catches a
//! variant that was named but never sampled.

use std::collections::BTreeSet;

use cctui_client::{Incoming, decode_frame};
use cctui_proto::api::{UserAction, UserActionKind, UserActionStatus};
use cctui_proto::github::{GithubEventKind, GithubEventPayload};
use cctui_proto::models::{MachineLiveness, SessionEndReason, SessionStatus};
use cctui_proto::resources::MachineResources;
use cctui_proto::ws::{AgentEvent, ScheduledLaunchState, ServerEvent};
use uuid::Uuid;

use crate::app::Action;
use crate::app::server_event::to_actions;

const VARIANT_COUNT: usize = 29;

fn uuid() -> Uuid {
    Uuid::nil()
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::DateTime::from_timestamp(0, 0).expect("epoch")
}

fn session() -> cctui_proto::models::Session {
    cctui_proto::models::Session {
        id: "s-1".to_owned(),
        parent_id: None,
        account_id: None,
        machine_id: "m-1".to_owned(),
        working_dir: "/home/dev/cctui".to_owned(),
        status: SessionStatus::Active,
        registered_at: now(),
        last_heartbeat: now(),
        metadata: serde_json::json!({"project_name": "cctui"}),
        adapter_id: None,
    }
}

fn user_action() -> UserAction {
    UserAction {
        id: uuid(),
        title: "Approve the PR".to_owned(),
        detail: None,
        kind: UserActionKind::Action,
        blocking: true,
        status: UserActionStatus::Open,
        note: None,
        created_at: now(),
        resolved_at: None,
        resolved_by: None,
    }
}

fn samples() -> Vec<ServerEvent> {
    let mut all = session_samples();
    all.extend(fleet_samples());
    all
}

fn session_samples() -> Vec<ServerEvent> {
    vec![
        ServerEvent::Stream {
            session_id: "s-1".to_owned(),
            data: AgentEvent::TurnEnd { ts: 1, seq: Some(1) },
        },
        ServerEvent::Status { session_id: "s-1".to_owned(), status: SessionStatus::Inactive },
        ServerEvent::SessionRegistered { session: session() },
        ServerEvent::SessionDeregistered { session_id: "s-1".to_owned() },
        ServerEvent::PermissionRequest {
            session_id: "s-1".to_owned(),
            request_id: "r-1".to_owned(),
            tool_name: "Bash".to_owned(),
            description: "run tests".to_owned(),
            input_preview: "cargo test".to_owned(),
        },
        ServerEvent::PermissionResolved {
            session_id: "s-1".to_owned(),
            request_id: "r-1".to_owned(),
        },
        ServerEvent::AskQuestion {
            session_id: "s-1".to_owned(),
            question: "which one?".to_owned(),
            questions: Some(serde_json::json!([{"header": "Pick"}])),
            preamble: Some("before".to_owned()),
        },
        ServerEvent::AskResolved { session_id: "s-1".to_owned() },
        ServerEvent::PlanRequest {
            session_id: "s-1".to_owned(),
            plan: "do the thing".to_owned(),
            preamble: None,
        },
        ServerEvent::PlanResolved { session_id: "s-1".to_owned() },
        ServerEvent::CommandResult {
            command_id: "c-1".to_owned(),
            ok: false,
            error: Some("nope".to_owned()),
            session_id: Some("s-1".to_owned()),
        },
        ServerEvent::SessionEnded {
            session_id: "s-1".to_owned(),
            reason: SessionEndReason::default(),
            detail: Some("clean exit".to_owned()),
        },
        ServerEvent::MessageAck {
            session_id: "s-1".to_owned(),
            client_msg_id: "cm-1".to_owned(),
            ok: true,
            error: None,
            command_id: Some(uuid()),
        },
    ]
}

fn fleet_samples() -> Vec<ServerEvent> {
    vec![
        ServerEvent::ArchiveManifest { machine_id: uuid(), count: 3 },
        ServerEvent::MachineLiveness { machine_id: uuid(), liveness: MachineLiveness::Online },
        ServerEvent::MachineResources {
            machine_id: uuid(),
            resources: MachineResources::default(),
        },
        ServerEvent::AccountUsage { account_id: uuid(), usage: serde_json::json!({"tokens": 10}) },
        ServerEvent::DispatcherLiveness {
            dispatcher_id: uuid(),
            liveness: MachineLiveness::Offline,
        },
        ServerEvent::ArchiveUploaded {
            machine_id: uuid(),
            project_dir: "/home/dev/cctui".to_owned(),
            session_id: "s-1".to_owned(),
            size_bytes: 1024,
            sha256: "abc".to_owned(),
        },
        ServerEvent::GithubEvent {
            kind: GithubEventKind::Pull,
            payload: GithubEventPayload {
                connector_id: uuid(),
                repo: "DorskFR/cctui".to_owned(),
                pull_number: Some(7),
            },
        },
        ServerEvent::SoftLimitReached {
            session_id: "s-1".to_owned(),
            account_id: uuid(),
            account_name: "primary".to_owned(),
            reason: "429".to_owned(),
            retry_after_secs: 60,
        },
        ServerEvent::SoftLimitCleared { session_id: "s-1".to_owned() },
        ServerEvent::ToolCallBlocked {
            session_id: "s-1".to_owned(),
            tool_name: "Bash".to_owned(),
            rule: "no-curl".to_owned(),
        },
        ServerEvent::PtyChunk { session_id: "s-1".to_owned(), data: "AAAA".to_owned() },
        ServerEvent::ScheduledLaunch {
            draft_id: "d-1".to_owned(),
            user_id: Some(uuid()),
            state: ScheduledLaunchState::Scheduled,
            launch_at: Some(now()),
            last_error: None,
        },
        ServerEvent::RoomMembers { room_id: uuid(), user_id: uuid() },
        ServerEvent::UserActions { session_id: "s-1".to_owned(), actions: vec![user_action()] },
        ServerEvent::Heartbeat {},
        ServerEvent::Resync { session_id: Some("s-1".to_owned()) },
    ]
}

fn variant_name(event: &ServerEvent) -> &'static str {
    match event {
        ServerEvent::Stream { .. } => "stream",
        ServerEvent::Status { .. } => "status",
        ServerEvent::SessionRegistered { .. } => "session_registered",
        ServerEvent::SessionDeregistered { .. } => "session_deregistered",
        ServerEvent::PermissionRequest { .. } => "permission_request",
        ServerEvent::PermissionResolved { .. } => "permission_resolved",
        ServerEvent::AskQuestion { .. } => "ask_question",
        ServerEvent::AskResolved { .. } => "ask_resolved",
        ServerEvent::PlanRequest { .. } => "plan_request",
        ServerEvent::PlanResolved { .. } => "plan_resolved",
        ServerEvent::CommandResult { .. } => "command_result",
        ServerEvent::SessionEnded { .. } => "session_ended",
        ServerEvent::MessageAck { .. } => "message_ack",
        ServerEvent::ArchiveManifest { .. } => "archive_manifest",
        ServerEvent::MachineLiveness { .. } => "machine_liveness",
        ServerEvent::MachineResources { .. } => "machine_resources",
        ServerEvent::AccountUsage { .. } => "account_usage",
        ServerEvent::DispatcherLiveness { .. } => "dispatcher_liveness",
        ServerEvent::ArchiveUploaded { .. } => "archive_uploaded",
        ServerEvent::GithubEvent { .. } => "github_event",
        ServerEvent::SoftLimitReached { .. } => "soft_limit_reached",
        ServerEvent::SoftLimitCleared { .. } => "soft_limit_cleared",
        ServerEvent::ToolCallBlocked { .. } => "tool_call_blocked",
        ServerEvent::PtyChunk { .. } => "pty_chunk",
        ServerEvent::ScheduledLaunch { .. } => "scheduled_launch",
        ServerEvent::RoomMembers { .. } => "room_members",
        ServerEvent::UserActions { .. } => "user_actions",
        ServerEvent::Heartbeat { .. } => "heartbeat",
        ServerEvent::Resync { .. } => "resync",
    }
}

#[test]
fn every_variant_has_a_sample() {
    let covered: BTreeSet<&str> = samples().iter().map(variant_name).collect();
    assert_eq!(
        covered.len(),
        VARIANT_COUNT,
        "one ServerEvent variant per sample: add the missing sample (and bump \
         VARIANT_COUNT) so the TUI's decode of it is covered",
    );
}

#[test]
fn every_variant_decodes_through_the_tui_receive_path() {
    for sample in samples() {
        let name = variant_name(&sample);
        let wire = serde_json::to_string(&sample)
            .unwrap_or_else(|e| panic!("{name} does not serialize: {e}"));
        match decode_frame(&wire) {
            Incoming::Event(decoded) => {
                assert_eq!(variant_name(&decoded), name, "{name} decoded as a different variant");
            }
            Incoming::Undecodable(reason) => {
                panic!("the TUI cannot decode {name}: {reason}\nwire: {wire}")
            }
            other @ (Incoming::Connected | Incoming::Disconnected(_)) => {
                panic!("a frame decoded as a socket lifecycle event: {other:?}")
            }
        }
    }
}

#[test]
fn no_variant_is_reported_as_undecodable_by_the_store() {
    for sample in samples() {
        let name = variant_name(&sample);
        for action in to_actions(sample) {
            assert!(
                !matches!(
                    action,
                    Action::UndecodableWsMessage(_) | Action::UndecodableAgentEvents(_)
                ),
                "{name} reached the store as undecodable",
            );
        }
    }
}

#[test]
fn an_unknown_variant_is_surfaced_rather_than_dropped() {
    let wire = serde_json::json!({"type": "not_a_real_event", "session_id": "s-1"}).to_string();
    match decode_frame(&wire) {
        Incoming::Undecodable(reason) => assert!(!reason.is_empty()),
        other => panic!("an unknown tag must surface as undecodable, got {other:?}"),
    }
}
