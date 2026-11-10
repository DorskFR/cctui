//! Refuses server frames whose features the server announced off, and holds
//! feature-owned frames that arrive before the announcement.

use std::collections::VecDeque;
use std::time::Duration;

use cctui_proto::features::FeatureSet;
use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp, ReadFileErrorKind};
use tokio::time::Instant;

/// How long after connect a feature-owned frame waits for the `Features`
/// frame. A server that sends none is served as every feature on after it.
pub const ANNOUNCE_GRACE: Duration = Duration::from_secs(3);

/// Past this many held frames the hold stops waiting and serves them.
const MAX_HELD: usize = 1024;

#[derive(Debug)]
pub enum Refusal {
    Reply(Box<DaemonFrameUp>),
    /// A failed `CommandResult` for the command's `command_id`.
    Command {
        adapter_id: String,
        command_id: uuid::Uuid,
        error: String,
    },
    Drop,
}

#[must_use]
pub fn served(frame: &DaemonFrameDown, features: FeatureSet) -> bool {
    let owners = frame.features();
    owners.is_empty() || owners.iter().any(|f| features.contains(*f))
}

/// `None` serves the frame.
#[must_use]
pub fn refusal(frame: &DaemonFrameDown, features: FeatureSet) -> Option<Refusal> {
    if served(frame, features) {
        return None;
    }
    let feature = frame.features().first()?;
    let command = match frame {
        DaemonFrameDown::Command { command, .. } => Some(command.kind_name()),
        _ => None,
    };
    tracing::debug!(%feature, ?command, "refusing a frame whose feature is off");
    Some(answer(frame, format!("{feature} is switched off on this server")))
}

fn answer(frame: &DaemonFrameDown, error: String) -> Refusal {
    let reply = match frame {
        DaemonFrameDown::ListDirs { request_id, .. } => DaemonFrameUp::ListDirsResult {
            request_id: *request_id,
            ok: false,
            dirs: Vec::new(),
            error: Some(error),
        },
        DaemonFrameDown::GitInfo { request_id, .. } => DaemonFrameUp::GitInfoResult {
            request_id: *request_id,
            ok: false,
            info: None,
            error: Some(error),
        },
        DaemonFrameDown::ReadFile { request_id, .. } => DaemonFrameUp::ReadFileResult {
            request_id: *request_id,
            ok: false,
            file: None,
            error_kind: Some(ReadFileErrorKind::Denied),
            error: Some(error),
            allowed_folders: Vec::new(),
        },
        DaemonFrameDown::StageFiles { request_id, .. } => DaemonFrameUp::StageFilesResult {
            request_id: *request_id,
            ok: false,
            paths: Vec::new(),
            error: Some(error),
        },
        DaemonFrameDown::PreviewRequest { stream_id, .. } => {
            DaemonFrameUp::PreviewError { stream_id: stream_id.clone(), error }
        }
        DaemonFrameDown::Command { adapter_id, command, .. } => {
            return command.command_id().map_or(Refusal::Drop, |command_id| Refusal::Command {
                adapter_id: adapter_id.clone(),
                command_id,
                error,
            });
        }
        _ => return Refusal::Drop,
    };
    Refusal::Reply(Box::new(reply))
}

/// The frames of one connection until its `Features` frame lands. Once a
/// frame is held every later one is held behind it, so the order survives.
#[derive(Debug)]
pub struct Hold {
    deadline: Instant,
    settled: bool,
    held: VecDeque<DaemonFrameDown>,
}

impl Hold {
    #[must_use]
    pub fn new(connected: Instant) -> Self {
        Self { deadline: connected + ANNOUNCE_GRACE, settled: false, held: VecDeque::new() }
    }

    /// The frames to handle now, in arrival order. A `Features` frame comes
    /// first, so the frames held behind it are judged on the announcement.
    pub fn admit(&mut self, frame: DaemonFrameDown, now: Instant) -> Vec<DaemonFrameDown> {
        if self.settled {
            return vec![frame];
        }
        if matches!(frame, DaemonFrameDown::Features { .. }) {
            self.settled = true;
            return std::iter::once(frame).chain(self.held.drain(..)).collect();
        }
        if now >= self.deadline || self.held.len() >= MAX_HELD {
            let mut frames = self.release();
            frames.push(frame);
            return frames;
        }
        if self.held.is_empty() && frame.features().is_empty() {
            return vec![frame];
        }
        self.held.push_back(frame);
        Vec::new()
    }

    /// Stop waiting for an announcement: the held frames, to serve as every
    /// feature on.
    pub fn release(&mut self) -> Vec<DaemonFrameDown> {
        self.settled = true;
        if !self.held.is_empty() {
            tracing::info!(
                held = self.held.len(),
                "the server announced no features; serving the held frames"
            );
        }
        self.held.drain(..).collect()
    }

    /// When to stop waiting; `None` while nothing is held.
    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.held.front().map(|_| self.deadline)
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::features::{Feature, FeatureSet};
    use cctui_proto::ws::{DaemonFrameDown, DaemonFrameUp, ReadFileErrorKind};
    use serde_json::{Value, json};

    use super::{ANNOUNCE_GRACE, Hold, Refusal, refusal};

    const ID: &str = "00000000-0000-0000-0000-000000000007";

    const fn request_id() -> uuid::Uuid {
        uuid::Uuid::from_u128(7)
    }

    fn down(value: Value) -> DaemonFrameDown {
        serde_json::from_value(value).unwrap()
    }

    fn command(command: Value) -> DaemonFrameDown {
        let mut frame = json!({ "type": "command", "adapter_id": "claude-code" });
        frame["command"] = command;
        down(frame)
    }

    fn spec() -> Value {
        json!({ "adapter_id": "claude-code", "working_dir": "/workspace" })
    }

    /// One frame of every switchable feature a server can address to a daemon.
    fn feature_frames() -> Vec<(Feature, DaemonFrameDown)> {
        let child = json!({ "adapter_id": "claude-code", "parent_local_id": "parent" });
        vec![
            (Feature::Files, down(json!({ "type": "list_dirs", "request_id": ID, "path": "/" }))),
            (Feature::Files, down(json!({ "type": "git_info", "request_id": ID, "path": "/" }))),
            (
                Feature::Files,
                down(
                    json!({ "type": "read_file", "request_id": ID, "path": "/a", "max_bytes": 1 }),
                ),
            ),
            (
                Feature::MachineSpawn,
                command(json!({ "kind": "spawn", "spec": spec(), "command_id": ID })),
            ),
            (
                Feature::MachineSpawn,
                command(json!({
                    "kind": "fork", "parent_local_id": "s", "spec": spec(), "command_id": ID
                })),
            ),
            (Feature::MachineSpawn, command(json!({ "kind": "resume", "local_id": "s" }))),
            (
                Feature::ChildSessions,
                command(json!({ "kind": "spawn", "spec": child, "command_id": ID })),
            ),
            (
                Feature::Terminal,
                command(json!({ "kind": "watch_pty", "local_id": "s", "watch": true })),
            ),
            (
                Feature::SelfUpdate,
                down(json!({
                    "type": "run_update_hook", "run_id": ID, "version": "1", "release_url": "u"
                })),
            ),
            (Feature::SelfUpdate, down(json!({ "type": "harness_update_policy", "policy": {} }))),
            (
                Feature::SelfUpdate,
                down(json!({ "type": "drain_restart", "harness": "claude-code" })),
            ),
            (
                Feature::Previews,
                down(json!({
                    "type": "preview_request", "stream_id": "x", "port": 5173,
                    "method": "GET", "path": "/"
                })),
            ),
            (
                Feature::Previews,
                down(json!({ "type": "preview_opened", "request_id": ID, "ok": true })),
            ),
            (
                Feature::Previews,
                down(json!({ "type": "preview_closed", "session_id": "s", "port": 5173 })),
            ),
            (Feature::Previews, down(json!({ "type": "preview_chunk", "stream_id": "x" }))),
            (Feature::Previews, down(json!({ "type": "preview_abort", "stream_id": "x" }))),
        ]
    }

    fn stage_files() -> DaemonFrameDown {
        down(json!({
            "type": "stage_files", "request_id": ID, "adapter_id": "claude-code",
            "local_id": "s", "uploads": []
        }))
    }

    /// Frames a core server sends, and every frame a dispatched worker gets
    /// over its life.
    fn core_frames() -> Vec<DaemonFrameDown> {
        vec![
            down(json!({ "type": "features", "off": ["files"] })),
            down(json!({ "type": "reconcile", "adapters": [] })),
            down(json!({ "type": "ack", "seq": 1 })),
            down(json!({ "type": "chunk_ack", "transfer_id": "t" })),
            down(json!({ "type": "resume_marks", "session_marks": [["s", 4]] })),
            down(json!({ "type": "transcript_ack", "adapter_id": "a", "session_marks": [] })),
            down(json!({ "type": "archived_jobs", "session_ids": ["s"] })),
            down(json!({ "type": "refresh_codex_models" })),
            command(json!({ "kind": "send_message", "local_id": "s", "text": "t" })),
            command(json!({ "kind": "kill", "local_id": "s" })),
            command(json!({
                "kind": "reply", "local_id": "s", "text": "t", "recover_auth": true,
                "env": { "ANTHROPIC_BASE_URL": "https://gateway.example" }, "command_id": ID
            })),
            command(json!({ "kind": "interrupt", "local_id": "s", "command_id": ID })),
            command(json!({
                "kind": "permission_response", "local_id": "s", "request_id": "r", "allow": true
            })),
            command(json!({ "kind": "rename", "local_id": "s", "name": "n" })),
            command(json!({ "kind": "remove", "local_id": "s", "command_id": ID })),
            command(
                json!({ "kind": "set_model", "local_id": "s", "model": "m", "command_id": ID }),
            ),
            command(json!({ "kind": "diagnose", "local_id": "s", "request_id": ID })),
            command(json!({ "kind": "watch_text", "local_id": "s", "watch": true })),
            command(json!({ "kind": "guard_transition", "local_id": "s", "step": "1" })),
            command(json!({ "kind": "resume_marks", "marks": [] })),
            command(json!({ "kind": "ack_marks", "marks": [] })),
        ]
    }

    fn assert_refused_with_its_reply(frame: &DaemonFrameDown, refusal: Refusal) {
        match refusal {
            Refusal::Reply(up) => assert_error_reply(frame, *up),
            Refusal::Command { adapter_id, command_id, error } => {
                let DaemonFrameDown::Command { adapter_id: sent, command, .. } = frame else {
                    panic!("{frame:?} refused with a command result");
                };
                assert_eq!(&adapter_id, sent);
                assert_eq!(Some(command_id), command.command_id(), "the reply is correlated");
                assert!(error.contains("switched off"));
            }
            Refusal::Drop => match frame {
                DaemonFrameDown::Command { command, .. } => {
                    assert!(command.command_id().is_none(), "a command with an id gets a reply");
                }
                DaemonFrameDown::RunUpdateHook { .. }
                | DaemonFrameDown::HarnessUpdatePolicy { .. }
                | DaemonFrameDown::DrainRestart { .. }
                | DaemonFrameDown::PreviewOpened { .. }
                | DaemonFrameDown::PreviewClosed { .. }
                | DaemonFrameDown::PreviewChunk(_)
                | DaemonFrameDown::PreviewAbort { .. } => {}
                _ => panic!("{frame:?} dropped without its reply"),
            },
        }
    }

    fn assert_error_reply(frame: &DaemonFrameDown, up: DaemonFrameUp) {
        match (frame, up) {
            (
                DaemonFrameDown::ListDirs { .. },
                DaemonFrameUp::ListDirsResult { request_id, ok, error, .. },
            )
            | (
                DaemonFrameDown::GitInfo { .. },
                DaemonFrameUp::GitInfoResult { request_id, ok, error, .. },
            )
            | (
                DaemonFrameDown::StageFiles { .. },
                DaemonFrameUp::StageFilesResult { request_id, ok, error, .. },
            ) => {
                assert_eq!(request_id, self::request_id(), "the reply is correlated");
                assert!(!ok);
                assert!(error.is_some_and(|e| e.contains("switched off")));
            }
            (
                DaemonFrameDown::ReadFile { .. },
                DaemonFrameUp::ReadFileResult { request_id, ok, error, error_kind, .. },
            ) => {
                assert_eq!(request_id, self::request_id());
                assert!(!ok);
                assert_eq!(error_kind, Some(ReadFileErrorKind::Denied));
                assert!(error.is_some_and(|e| e.contains("switched off")));
            }
            (
                DaemonFrameDown::PreviewRequest { .. },
                DaemonFrameUp::PreviewError { stream_id, error },
            ) => {
                assert_eq!(stream_id, "x");
                assert!(error.contains("previews is switched off"));
            }
            (frame, up) => panic!("{frame:?} refused with {up:?}"),
        }
    }

    #[test]
    fn each_features_frames_are_refused_when_it_is_announced_off() {
        for (feature, frame) in feature_frames() {
            let off = FeatureSet::all().without(feature);
            let reply = refusal(&frame, off).unwrap_or_else(|| panic!("served {frame:?}"));
            assert_refused_with_its_reply(&frame, reply);
        }
    }

    #[test]
    fn each_features_frames_are_served_when_it_is_on_or_never_announced() {
        let unannounced = crate::serverfeatures::from_off(&[]);
        for (feature, frame) in feature_frames() {
            let others_off = FeatureSet::none().with(feature);
            for features in [FeatureSet::all(), unannounced, others_off] {
                assert!(refusal(&frame, features).is_none(), "refused {frame:?} under {features}");
            }
        }
    }

    #[test]
    fn staging_files_is_refused_only_when_uploads_and_context_are_both_off() {
        let frame = stage_files();
        let both_off = FeatureSet::all().without(Feature::Uploads).without(Feature::Context);
        let reply = refusal(&frame, both_off).expect("refused");
        assert_refused_with_its_reply(&frame, reply);
        for on in [Feature::Uploads, Feature::Context] {
            assert!(refusal(&frame, FeatureSet::none().with(on)).is_none(), "{on} serves it");
        }
    }

    #[test]
    fn a_child_session_spawn_is_served_with_machine_spawn_off() {
        let child = command(json!({
            "kind": "spawn", "command_id": ID,
            "spec": { "adapter_id": "codex", "parent_local_id": "parent" }
        }));
        let features = FeatureSet::all().without(Feature::MachineSpawn);
        assert!(refusal(&child, features).is_none());
    }

    #[test]
    fn a_dispatch_launch_is_served_with_every_switchable_feature_off() {
        // The worker's own launch is read from `TASK_PAYLOAD_JSON` when its
        // adapter starts, never sent as a frame; what reaches it over the
        // socket is the `Reconcile` that starts that adapter and the core
        // frames of its session.
        for frame in core_frames() {
            assert!(refusal(&frame, FeatureSet::none()).is_none(), "refused {frame:?}");
        }
    }

    #[test]
    fn core_frames_are_never_refused() {
        let mut sets = vec![FeatureSet::none(), FeatureSet::all()];
        sets.extend(Feature::ALL.into_iter().map(|f| FeatureSet::all().without(f)));
        for features in sets {
            for frame in core_frames() {
                assert!(refusal(&frame, features).is_none(), "refused {frame:?} under {features}");
            }
        }
    }

    fn now() -> tokio::time::Instant {
        tokio::time::Instant::now()
    }

    const fn is_features(frame: &DaemonFrameDown) -> bool {
        matches!(frame, DaemonFrameDown::Features { .. })
    }

    #[test]
    fn the_hold_passes_everything_when_the_features_frame_comes_first() {
        let start = now();
        let mut hold = Hold::new(start);
        let first = hold.admit(down(json!({ "type": "features", "off": [] })), start);
        assert!(first.len() == 1 && is_features(&first[0]));
        let spawn = command(json!({ "kind": "spawn", "spec": spec() }));
        assert_eq!(hold.admit(spawn, start).len(), 1);
        assert!(hold.deadline().is_none());
    }

    #[test]
    fn a_feature_frame_before_the_announcement_is_held_and_judged_on_it() {
        let start = now();
        let mut hold = Hold::new(start);
        let reconcile = down(json!({ "type": "reconcile", "adapters": [] }));
        assert_eq!(hold.admit(reconcile, start).len(), 1, "a core frame with nothing held passes");
        let pty = command(json!({ "kind": "watch_pty", "local_id": "s", "watch": true }));
        assert!(hold.admit(pty, start).is_empty());
        let kill = command(json!({ "kind": "kill", "local_id": "s" }));
        assert!(hold.admit(kill, start).is_empty(), "a core frame queues behind a held one");
        assert_eq!(hold.deadline(), Some(start + ANNOUNCE_GRACE));
        let released = hold.admit(down(json!({ "type": "features", "off": ["terminal"] })), start);
        assert_eq!(released.len(), 3);
        assert!(is_features(&released[0]), "the announcement is handled first");
        assert!(matches!(&released[1], DaemonFrameDown::Command { command, .. }
            if command.kind_name() == "watch_pty"));
        assert!(matches!(&released[2], DaemonFrameDown::Command { command, .. }
            if command.kind_name() == "kill"));
        let off = crate::serverfeatures::from_off(&["terminal".to_owned()]);
        assert!(refusal(&released[1], off).is_some());
        assert!(hold.deadline().is_none());
    }

    #[test]
    fn a_server_that_never_announces_gets_its_held_frames_served_after_the_grace() {
        let start = now();
        let mut hold = Hold::new(start);
        let spawn = command(json!({ "kind": "spawn", "spec": spec() }));
        assert!(hold.admit(spawn, start).is_empty());
        assert_eq!(hold.release().len(), 1);
        let late = command(json!({ "kind": "watch_pty", "local_id": "s", "watch": true }));
        assert_eq!(hold.admit(late, start).len(), 1, "a settled hold holds nothing");

        let mut hold = Hold::new(start);
        let early = command(json!({ "kind": "resume", "local_id": "s" }));
        assert!(hold.admit(early, start).is_empty());
        let after = command(json!({ "kind": "resume", "local_id": "t" }));
        assert_eq!(hold.admit(after, start + ANNOUNCE_GRACE).len(), 2, "past the grace");
    }
}
