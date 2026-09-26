//! Daemon ↔ server adapter wire types. The async `Adapter` trait lives in
//! `cctui-daemon` so this crate stays free of tokio.

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// Adapter implementation id, e.g. `claude-code`, `codex`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(transparent)]
pub struct AdapterId(pub String);

/// Every adapter compiled into the daemon. `adapters_enabled` rows only override
/// config or disable; a missing binary surfaces as a failed spawn.
pub const KNOWN_ADAPTERS: &[&str] = &["claude-code", "codex", "opencode"];

impl AdapterId {
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for AdapterId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for AdapterId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl std::fmt::Display for AdapterId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Adapter-specific session metadata, stored as JSONB.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMeta {
    pub working_dir: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_local_id: Option<String>,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

impl Default for SessionMeta {
    fn default() -> Self {
        Self { working_dir: None, parent_local_id: None, extra: serde_json::Value::Null }
    }
}

/// 8-hex `claude daemon` worker shortcode, `^[0-9a-f]{8}$`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JobShort(String);

impl JobShort {
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        if s.len() == 8 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
            Some(Self(s.to_string()))
        } else {
            None
        }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for JobShort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EndReason {
    Completed,
    Killed,
    Crashed {
        detail: String,
    },
    /// The adapter could not revive an existing session (`thread/resume`).
    ResumeFailed {
        detail: String,
    },
    /// A fresh spawn never reached a running session.
    SpawnFailed {
        detail: String,
    },
    Other {
        detail: String,
    },
}

impl EndReason {
    #[must_use]
    pub const fn kind(&self) -> crate::models::SessionEndReason {
        match self {
            Self::Completed => crate::models::SessionEndReason::Completed,
            Self::Killed => crate::models::SessionEndReason::Killed,
            Self::Crashed { .. } => crate::models::SessionEndReason::Crashed,
            Self::ResumeFailed { .. } => crate::models::SessionEndReason::ResumeFailed,
            Self::SpawnFailed { .. } => crate::models::SessionEndReason::SpawnFailed,
            Self::Other { .. } => crate::models::SessionEndReason::Other,
        }
    }

    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        match self {
            Self::Completed | Self::Killed => None,
            Self::Crashed { detail }
            | Self::ResumeFailed { detail }
            | Self::SpawnFailed { detail }
            | Self::Other { detail } => Some(detail),
        }
    }
}

/// Adapter → server events. `local_id` is the adapter's own session id, resolved
/// server-side via `(machine_id, adapter_id, local_id)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum AdapterEvent {
    SessionStarted {
        local_id: String,
        meta: SessionMeta,
    },
    Message {
        local_id: String,
        payload: serde_json::Value,
        /// Client-minted turn id. `None` for turns cctui did not originate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<Uuid>,
    },
    ToolUse {
        local_id: String,
        payload: serde_json::Value,
    },
    SessionEnded {
        local_id: String,
        reason: EndReason,
    },
    /// Runtime status snapshot; every field but `local_id` is optional.
    Status {
        local_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tempo: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        activity: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        intent: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        /// Posture observed in the transcript.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission_mode: Option<PermissionMode>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        children: Vec<SessionChild>,
    },
    /// Fallback PR children; applied only when the session has none.
    PrLink {
        local_id: String,
        children: Vec<SessionChild>,
    },
    /// Per-assistant-message usage, idempotent on `(session_id, message_id)`.
    /// Cache fields are `0` when unreported.
    TokenUsage {
        local_id: String,
        message_id: String,
        input_tokens: u64,
        output_tokens: u64,
        #[serde(default)]
        cache_read_tokens: u64,
        #[serde(default)]
        cache_creation_tokens: u64,
    },
    /// Model observed on an assistant message. Written only when the session
    /// model is unset.
    SessionModel {
        local_id: String,
        model: String,
    },
    /// Blocked on a tool-permission decision; answer with
    /// [`AdapterCommand::PermissionResponse`] echoing `request_id`.
    PermissionRequest {
        local_id: String,
        request_id: String,
        tool: String,
        #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
        input: serde_json::Value,
    },
    /// The permission prompt was answered or dismissed elsewhere.
    PermissionResolved {
        local_id: String,
        request_id: String,
    },
    /// Pending `AskUserQuestion`, delivered by the `PreToolUse` hook because the
    /// transcript only shows it after the answer. Answer with [`AdapterCommand::Reply`].
    AskQuestion {
        local_id: String,
        question: String,
        /// Raw `tool_input.questions`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        questions: Option<serde_json::Value>,
        /// Assistant text preceding the tool call in the same turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preamble: Option<String>,
    },
    AskResolved {
        local_id: String,
    },
    /// Pending `ExitPlanMode` approval, delivered by the `PreToolUse` hook.
    /// Answer with [`AdapterCommand::Reply`]: a digit 1–3 or free text.
    PlanRequest {
        local_id: String,
        plan: String,
        /// Assistant prose preceding the `ExitPlanMode` call in the same turn,
        /// read from the transcript by the `ask-hook` subcommand. `None` when
        /// the model called the tool with no preceding text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preamble: Option<String>,
    },
    PlanResolved {
        local_id: String,
    },
    /// Outcome of a server-initiated command, correlated by `command_id`.
    CommandResult {
        command_id: Uuid,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Reply to [`AdapterCommand::Diagnose`], correlated by `request_id`.
    Diagnose {
        local_id: String,
        request_id: Uuid,
        report: Box<crate::diagnose::SessionDiagnose>,
    },
    /// Machine-scoped codex `model/list` catalog.
    CodexModels {
        catalog: crate::codex_catalog::CodexModelCatalog,
    },
    /// Base64 PTY bytes, sent only while [`AdapterCommand::WatchPty`] is on.
    /// Never persisted.
    PtyChunk {
        local_id: String,
        data: String,
    },
    /// Transcript byte-offset high-water mark, returned as a resume mark on reconnect.
    TranscriptMark {
        local_id: String,
        offset: u64,
    },
    /// Rate-limit windows the agent reported for its bound credential.
    RateLimits {
        local_id: String,
        windows: Vec<RateLimitWindow>,
        /// Unix seconds. `None` = now.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        observed_at: Option<i64>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RateLimitWindow {
    pub used_percent: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_minutes: Option<i64>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
    /// `primary` or `secondary`.
    pub slot: String,
}

/// Child reference, typically a linked PR.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionChild {
    pub id: String,
    pub href: String,
    pub kind: String,
}

/// Daemon → adapter commands.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AdapterCommand {
    /// `local_id` → stored transcript byte offset; tail cursors clamp forward.
    ResumeMarks {
        marks: Vec<(String, u64)>,
    },
    SendMessage {
        local_id: String,
        text: String,
    },
    Kill {
        local_id: String,
        /// `None` = SIGTERM.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signal: Option<i32>,
    },
    Spawn {
        spec: SessionSpec,
        /// Echoed in [`AdapterEvent::CommandResult`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<Uuid>,
        /// Pre-minted id the gateway token is bound to. Ignored by adapters that mint
        /// their own.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<Uuid>,
    },
    /// Fork into a new session; the parent is left intact and linked via
    /// [`SessionMeta::parent_local_id`].
    Fork {
        parent_local_id: String,
        spec: SessionSpec,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<Uuid>,
        /// Pre-minted child id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// `None` = full history. Claude only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        extract: Option<ForkExtract>,
    },
    /// Inject `text` into the worker as a user turn.
    Reply {
        local_id: String,
        text: String,
        /// 0-based option picks per question for a pending `AskUserQuestion`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ask_picks: Option<Vec<Vec<usize>>>,
        /// Fresh gateway env, used only if the reply cold-resumes the worker.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        env: std::collections::BTreeMap<String, String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<Uuid>,
        /// Stamped onto every resulting [`AdapterEvent::Message`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<Uuid>,
    },
    /// Stop the in-flight turn; the session stays live.
    Interrupt {
        local_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<Uuid>,
    },
    /// Revive an exited conversation without replying. `working_dir` covers a
    /// removed job `state.json`.
    Resume {
        local_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
        /// Fresh gateway env for the revived worker.
        #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
        env: std::collections::BTreeMap<String, String>,
    },
    PermissionResponse {
        local_id: String,
        request_id: String,
        allow: bool,
    },
    /// Persist the name to the adapter's own source of truth.
    Rename {
        local_id: String,
        name: String,
    },
    /// Stop the worker and delete its job metadata (`claude rm`); the transcript
    /// stays resumable. Adapters without one treat it as a kill.
    Remove {
        local_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<Uuid>,
        /// A claude job cctui did not start is removed only on [`RemoveInitiator::User`].
        #[serde(default)]
        initiator: RemoveInitiator,
    },
    /// Change model and/or effort in place. Claude rejects it; fork instead.
    SetModel {
        local_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<Uuid>,
    },
    /// Read-only snapshot, answered by [`AdapterEvent::Diagnose`].
    Diagnose {
        local_id: String,
        request_id: Uuid,
    },
    /// Start or stop relaying [`AdapterEvent::PtyChunk`]. Never injects input;
    /// ignored by adapters without a PTY.
    WatchPty {
        local_id: String,
        watch: bool,
    },
}

impl AdapterCommand {
    /// `None` for adapter-wide commands.
    #[must_use]
    pub fn local_id(&self) -> Option<&str> {
        match self {
            Self::SendMessage { local_id, .. }
            | Self::Kill { local_id, .. }
            | Self::Reply { local_id, .. }
            | Self::Interrupt { local_id, .. }
            | Self::Resume { local_id, .. }
            | Self::PermissionResponse { local_id, .. }
            | Self::Rename { local_id, .. }
            | Self::Remove { local_id, .. }
            | Self::SetModel { local_id, .. }
            | Self::Diagnose { local_id, .. }
            | Self::WatchPty { local_id, .. } => Some(local_id),
            Self::Fork { parent_local_id, .. } => Some(parent_local_id),
            Self::Spawn { spec, .. } => spec.parent_local_id.as_deref(),
            Self::ResumeMarks { .. } => None,
        }
    }

    #[must_use]
    pub const fn command_id(&self) -> Option<Uuid> {
        match self {
            Self::Spawn { command_id, .. }
            | Self::Fork { command_id, .. }
            | Self::Reply { command_id, .. }
            | Self::Interrupt { command_id, .. }
            | Self::Remove { command_id, .. }
            | Self::SetModel { command_id, .. } => *command_id,
            _ => None,
        }
    }
}

/// Defaults to [`Self::Automatic`], the conservative reading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum RemoveInitiator {
    User,
    /// TTL sweep, spawn-intent auto-archive or reconcile purge.
    #[default]
    Automatic,
}

impl RemoveInitiator {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Automatic => "automatic",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" => Some(Self::User),
            "automatic" => Some(Self::Automatic),
            _ => None,
        }
    }
}

/// Subset-fork slice, anchored on assistant `message_id`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ForkMode {
    /// Up to and including the anchor.
    UpTo,
    /// Strictly after the anchor.
    After,
    /// Only turns containing the selected messages.
    Selected,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ForkExtract {
    pub mode: ForkMode,
    /// For `up_to` / `after`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_message_id: Option<String>,
    /// For `selected`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub selected_message_ids: Vec<String>,
}

/// Per-spawn permission posture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    /// No prompts, no sandbox.
    Yolo,
    /// No prompts, sandboxed.
    Auto,
    /// Prompt on every action.
    Ask,
    /// Yolo, plus `AskUserQuestion` denied and a `Stop` hook against stalling.
    /// Codex treats it as yolo.
    Whip,
}

impl PermissionMode {
    /// ask < auto < yolo = whip.
    #[must_use]
    pub const fn rank(self) -> u8 {
        match self {
            Self::Ask => 0,
            Self::Auto => 1,
            Self::Yolo | Self::Whip => 2,
        }
    }

    #[must_use]
    pub const fn within(self, ceiling: Self) -> bool {
        self.rank() <= ceiling.rank()
    }

    /// The less permissive of `a` and `b`, `a` on a tie.
    #[must_use]
    pub const fn stricter(a: Self, b: Self) -> Self {
        if b.rank() < a.rank() { b } else { a }
    }

    /// Accepts serde names and claude `--permission-mode` values; `plan` is `ask`.
    #[must_use]
    pub fn from_session_label(s: &str) -> Option<Self> {
        match s.trim() {
            "ask" | "default" | "plan" => Some(Self::Ask),
            "auto" | "acceptEdits" => Some(Self::Auto),
            "yolo" | "bypassPermissions" => Some(Self::Yolo),
            "whip" => Some(Self::Whip),
            _ => None,
        }
    }

    #[must_use]
    pub const fn claude_flag(self) -> &'static str {
        match self {
            Self::Yolo | Self::Whip => "bypassPermissions",
            Self::Auto => "acceptEdits",
            Self::Ask => "default",
        }
    }

    /// `(sandbox_mode, approval_policy)`.
    #[must_use]
    pub const fn codex_sandbox_approval(self) -> (&'static str, &'static str) {
        match self {
            Self::Yolo | Self::Whip => ("danger-full-access", "never"),
            Self::Auto => ("workspace-write", "never"),
            Self::Ask => ("workspace-write", "on-request"),
        }
    }

    #[must_use]
    pub const fn is_whip(self) -> bool {
        matches!(self, Self::Whip)
    }

    /// `default` / `auto` / `yolo`, as shown in `<session-context>`.
    #[must_use]
    pub const fn normalized_label(self) -> &'static str {
        match self {
            Self::Ask => "default",
            Self::Auto => "auto",
            Self::Yolo | Self::Whip => "yolo",
        }
    }
}

/// Spawn parameters. `Debug` redacts `env` and `bootstrap`.
#[derive(Clone, Serialize, Deserialize)]
pub struct SessionSpec {
    pub adapter_id: AdapterId,
    pub working_dir: Option<String>,
    pub prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// `None` = the daemon's per-host default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<PermissionMode>,
    /// `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// `None` = adapter default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Codex only, always concrete: `default` or `fast`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Never persisted or logged.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub env: std::collections::BTreeMap<String, String>,
    /// [`BootstrapUploads`] JSON when present.
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub bootstrap: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_local_id: Option<String>,
}

impl std::fmt::Debug for SessionSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionSpec")
            .field("adapter_id", &self.adapter_id)
            .field("working_dir", &self.working_dir)
            .field("prompt", &self.prompt)
            .field("name", &self.name)
            .field("permission_mode", &self.permission_mode)
            .field("effort", &self.effort)
            .field("model", &self.model)
            .field("service_tier", &self.service_tier)
            .field("env", &format_args!("<{} secret(s) redacted>", self.env.len()))
            .field("bootstrap", &format_args!("<redacted>"))
            .field("parent_local_id", &self.parent_local_id)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapUploads {
    pub uploads: Vec<BootstrapFile>,
}

/// `name` is a bare filename; `content_b64` is standard base64.
#[derive(Clone, Serialize, Deserialize)]
pub struct BootstrapFile {
    pub name: String,
    pub content_b64: String,
}

impl std::fmt::Debug for BootstrapFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BootstrapFile")
            .field("name", &self.name)
            .field("content_b64", &format_args!("<{} b64 bytes>", self.content_b64.len()))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_permission_mode_maps_to_an_approval_policy_codex_accepts() {
        for mode in
            [PermissionMode::Yolo, PermissionMode::Whip, PermissionMode::Auto, PermissionMode::Ask]
        {
            let (_, approval) = mode.codex_sandbox_approval();
            assert!(matches!(approval, "on-request" | "never"), "{mode:?} → {approval}");
        }
    }

    #[test]
    fn remove_carries_its_correlation_id() {
        let id = Uuid::new_v4();
        let cmd = AdapterCommand::Remove {
            local_id: "sess-1".into(),
            command_id: Some(id),
            initiator: RemoveInitiator::User,
        };
        assert_eq!(cmd.command_id(), Some(id));
        assert_eq!(cmd.local_id(), Some("sess-1"));
        let back: AdapterCommand =
            serde_json::from_str(&serde_json::to_string(&cmd).unwrap()).unwrap();
        assert_eq!(back.command_id(), Some(id));
        assert!(matches!(back, AdapterCommand::Remove { initiator: RemoveInitiator::User, .. }));
    }

    #[test]
    fn remove_without_an_initiator_deserializes_as_automatic() {
        let cmd: AdapterCommand =
            serde_json::from_str(r#"{"kind":"remove","local_id":"sess-1"}"#).unwrap();
        assert!(matches!(
            cmd,
            AdapterCommand::Remove { initiator: RemoveInitiator::Automatic, .. }
        ));
    }

    fn spec(parent: Option<&str>) -> SessionSpec {
        SessionSpec {
            adapter_id: AdapterId::new("codex"),
            working_dir: Some("/workspace".into()),
            prompt: Some("review".into()),
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            service_tier: None,
            env: std::collections::BTreeMap::new(),
            bootstrap: serde_json::Value::Null,
            parent_local_id: parent.map(str::to_owned),
        }
    }

    #[test]
    fn a_child_spawn_belongs_to_its_parent_session() {
        let cmd = AdapterCommand::Spawn {
            spec: spec(Some("parent-1")),
            command_id: None,
            session_id: Some(Uuid::new_v4()),
        };
        assert_eq!(cmd.local_id(), Some("parent-1"));
    }

    #[test]
    fn a_top_level_spawn_has_no_session() {
        let cmd = AdapterCommand::Spawn { spec: spec(None), command_id: None, session_id: None };
        assert_eq!(cmd.local_id(), None);
    }

    #[test]
    fn remove_from_older_peer_has_no_correlation_id() {
        let json = r#"{"kind":"remove","local_id":"sess-1"}"#;
        let cmd: AdapterCommand = serde_json::from_str(json).unwrap();
        assert_eq!(cmd.command_id(), None);
    }

    #[test]
    fn adapter_id_roundtrips() {
        let id = AdapterId::new("claude-code");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""claude-code""#);
        let back: AdapterId = serde_json::from_str(&json).unwrap();
        assert_eq!(back.as_str(), "claude-code");
    }

    #[test]
    fn adapter_event_codex_models_roundtrips() {
        let evt = AdapterEvent::CodexModels {
            catalog: crate::codex_catalog::CodexModelCatalog {
                models: vec![crate::codex_catalog::CodexModel {
                    id: "gpt-5.6-sol".into(),
                    model: "gpt-5.6-sol".into(),
                    display_name: "GPT-5.6 Sol".into(),
                    description: String::new(),
                    hidden: false,
                    is_default: true,
                    supported_efforts: vec!["low".into(), "high".into()],
                    default_effort: "medium".into(),
                    input_modalities: vec!["text".into()],
                    upgrade: None,
                    minimal_client_version: None,
                }],
                client_version: None,
            },
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""kind":"codex_models""#));
        let back: AdapterEvent = serde_json::from_str(&json).unwrap();
        let AdapterEvent::CodexModels { catalog } = back else { panic!("wrong variant") };
        assert_eq!(catalog.models[0].id, "gpt-5.6-sol");
        assert_eq!(catalog.models[0].supported_efforts, ["low", "high"]);
    }

    #[test]
    fn adapter_event_session_started_roundtrips() {
        let evt = AdapterEvent::SessionStarted {
            local_id: "abc".into(),
            meta: SessionMeta { working_dir: Some("/tmp".into()), ..SessionMeta::default() },
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""kind":"session_started""#));
        let back: AdapterEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AdapterEvent::SessionStarted { .. }));
    }

    #[test]
    fn adapter_event_all_variants_roundtrip() {
        let cases = vec![
            AdapterEvent::SessionStarted { local_id: "s1".into(), meta: SessionMeta::default() },
            AdapterEvent::Message {
                local_id: "s1".into(),
                payload: serde_json::json!({"role": "assistant", "text": "hi"}),
                turn_id: None,
            },
            AdapterEvent::ToolUse {
                local_id: "s1".into(),
                payload: serde_json::json!({"tool": "Bash", "input": {}}),
            },
            AdapterEvent::SessionEnded { local_id: "s1".into(), reason: EndReason::Completed },
        ];
        for evt in cases {
            let json = serde_json::to_string(&evt).unwrap();
            let _back: AdapterEvent = serde_json::from_str(&json).expect(&json);
        }
    }

    #[test]
    fn message_turn_id_roundtrips_and_is_omitted_when_absent() {
        let bare = AdapterEvent::Message {
            local_id: "s1".into(),
            payload: serde_json::json!({"role": "user", "text": "hi"}),
            turn_id: None,
        };
        let json = serde_json::to_string(&bare).unwrap();
        assert!(!json.contains("turn_id"), "{json}");
        let back: AdapterEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AdapterEvent::Message { turn_id: None, .. }));

        let id = Uuid::new_v4();
        let stamped = AdapterEvent::Message {
            local_id: "s1".into(),
            payload: serde_json::json!({"role": "user", "text": "hi"}),
            turn_id: Some(id),
        };
        let json = serde_json::to_string(&stamped).unwrap();
        let back: AdapterEvent = serde_json::from_str(&json).unwrap();
        let AdapterEvent::Message { turn_id, .. } = back else { panic!("wrong variant") };
        assert_eq!(turn_id, Some(id));
    }

    #[test]
    fn message_from_daemon_predating_the_field_decodes() {
        let legacy = r#"{"kind":"message","local_id":"s1","payload":{"role":"user"}}"#;
        let back: AdapterEvent = serde_json::from_str(legacy).unwrap();
        assert!(matches!(back, AdapterEvent::Message { turn_id: None, .. }));
    }

    #[test]
    fn reply_command_carries_turn_id() {
        let id = Uuid::new_v4();
        let cmd = AdapterCommand::Reply {
            local_id: "s1".into(),
            text: "go on".into(),
            ask_picks: None,
            env: std::collections::BTreeMap::default(),
            command_id: None,
            turn_id: Some(id),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let back: AdapterCommand = serde_json::from_str(&json).unwrap();
        let AdapterCommand::Reply { turn_id, .. } = back else { panic!("wrong variant") };
        assert_eq!(turn_id, Some(id));

        let legacy = r#"{"kind":"reply","local_id":"s1","text":"go on"}"#;
        let back: AdapterCommand = serde_json::from_str(legacy).unwrap();
        assert!(matches!(back, AdapterCommand::Reply { turn_id: None, .. }));
    }

    #[test]
    fn adapter_command_roundtrips() {
        let cmd = AdapterCommand::SendMessage { local_id: "s1".into(), text: "hello".into() };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""kind":"send_message""#));
        let back: AdapterCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AdapterCommand::SendMessage { .. }));
    }

    #[test]
    fn end_reason_crashed_roundtrips() {
        let r = EndReason::Crashed { detail: "oom".into() };
        let json = serde_json::to_string(&r).unwrap();
        let back: EndReason = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn job_short_parses_and_rejects() {
        assert!(JobShort::parse("6e189420").is_some());
        assert!(JobShort::parse("6E189420").is_none(), "must be lowercase");
        assert!(JobShort::parse("6e18942").is_none(), "must be 8 chars");
        assert!(JobShort::parse("6e189420x").is_none(), "non-hex rejected");
        let s = JobShort::parse("6e189420").unwrap();
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(json, r#""6e189420""#);
        let back: JobShort = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn adapter_event_status_roundtrips() {
        let evt = AdapterEvent::Status {
            local_id: "s1".into(),
            tempo: Some("active".into()),
            state: Some("working".into()),
            detail: Some("running tests".into()),
            activity: None,
            name: Some("DEFI-1317".into()),
            intent: None,
            model: Some("opus[1m]".into()),
            effort: Some("low".into()),
            permission_mode: None,
            children: vec![SessionChild {
                id: "1972".into(),
                href: "https://github.com/o/r/pull/1972".into(),
                kind: "pr".into(),
            }],
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""kind":"status""#));
        let back: AdapterEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AdapterEvent::Status { .. }));
    }

    #[test]
    fn adapter_event_status_minimal_roundtrips() {
        let evt = AdapterEvent::Status {
            local_id: "s1".into(),
            tempo: None,
            state: None,
            detail: None,
            activity: None,
            name: None,
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: vec![],
        };
        let json = serde_json::to_string(&evt).unwrap();
        // Optional fields with `skip_serializing_if` drop out cleanly.
        assert!(!json.contains("tempo"));
        assert!(!json.contains("children"));
        let _back: AdapterEvent = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn adapter_event_permission_request_roundtrips() {
        let evt = AdapterEvent::PermissionRequest {
            local_id: "s1".into(),
            request_id: "req-123".into(),
            tool: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""kind":"permission_request""#));
        let _back: AdapterEvent = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn adapter_command_reply_kill_perm_roundtrip() {
        let cases = vec![
            AdapterCommand::Reply {
                local_id: "s1".into(),
                text: "go on".into(),
                ask_picks: None,
                env: std::collections::BTreeMap::default(),
                command_id: None,
                turn_id: None,
            },
            AdapterCommand::Kill { local_id: "s1".into(), signal: Some(15) },
            AdapterCommand::Kill { local_id: "s1".into(), signal: None },
            AdapterCommand::PermissionResponse {
                local_id: "s1".into(),
                request_id: "req-123".into(),
                allow: true,
            },
        ];
        for cmd in cases {
            let json = serde_json::to_string(&cmd).unwrap();
            let _back: AdapterCommand = serde_json::from_str(&json).expect(&json);
        }
    }

    #[test]
    fn adapter_command_kill_signal_omitted_by_default() {
        let cmd = AdapterCommand::Kill { local_id: "s1".into(), signal: None };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("signal"), "signal:None must not serialize");
        let back: AdapterCommand =
            serde_json::from_str(r#"{"kind":"kill","local_id":"s1"}"#).unwrap();
        assert!(matches!(back, AdapterCommand::Kill { signal: None, .. }));
    }

    #[test]
    fn adapter_diagnose_command_and_event_roundtrip() {
        // the diagnose round-trip rides the generic Command/Event
        // path, so its serde shape must stay wire-stable.
        let req_id = Uuid::new_v4();
        let cmd = AdapterCommand::Diagnose { local_id: "s1".into(), request_id: req_id };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""kind":"diagnose""#));
        let back: AdapterCommand = serde_json::from_str(&json).unwrap();
        assert!(
            matches!(back, AdapterCommand::Diagnose { request_id, .. } if request_id == req_id)
        );

        let report = crate::diagnose::SessionDiagnose {
            local_id: "s1".into(),
            short: None,
            generated_at_ms: 42,
            adapter: "claude-code".into(),
            effective_state: crate::diagnose::DiagnoseFact::missing("activity", "no status"),
            last_hook_event: crate::diagnose::DiagnoseFact::missing("hook", "none"),
            attach: crate::diagnose::DiagnoseFact::missing("attach", "none"),
            pty_output: crate::diagnose::DiagnoseFact::missing(
                "pty",
                "PTY capture not implemented",
            ),
            claude_socket: crate::diagnose::DiagnoseFact::missing("discovery", "none"),
            transcript: crate::diagnose::DiagnoseFact::missing("filesystem", "none"),
            prompts: crate::diagnose::DiagnoseFact::missing("hook", "none"),
            permission_mode: crate::diagnose::DiagnoseFact::missing("spawn", "none"),
            dispatch: crate::diagnose::DiagnoseFact::missing("dispatch", "none"),
            gateway: crate::diagnose::DiagnoseFact::missing("daemon-config", "none"),
            codex: None,
        };
        let evt = AdapterEvent::Diagnose {
            local_id: "s1".into(),
            request_id: req_id,
            report: Box::new(report.clone()),
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""kind":"diagnose""#));
        let back: AdapterEvent = serde_json::from_str(&json).unwrap();
        match back {
            AdapterEvent::Diagnose { request_id, report: r, .. } => {
                assert_eq!(request_id, req_id);
                assert_eq!(*r, report);
            }
            other => panic!("expected Diagnose, got {other:?}"),
        }
    }

    #[test]
    fn remove_initiator_persists_as_its_wire_name() {
        for i in [RemoveInitiator::User, RemoveInitiator::Automatic] {
            assert_eq!(serde_json::to_value(i).unwrap(), serde_json::json!(i.as_str()));
            assert_eq!(RemoveInitiator::parse(i.as_str()), Some(i));
        }
        assert_eq!(RemoveInitiator::parse("reaper"), None);
    }

    #[test]
    fn watch_pty_command_and_pty_chunk_event_roundtrip() {
        let cmd = AdapterCommand::WatchPty { local_id: "s1".into(), watch: true };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""kind":"watch_pty""#));
        assert!(json.contains(r#""watch":true"#));
        let back: AdapterCommand = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, AdapterCommand::WatchPty { watch: true, .. }));

        let evt = AdapterEvent::PtyChunk { local_id: "s1".into(), data: "aGk=".into() };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains(r#""kind":"pty_chunk""#));
        let back: AdapterEvent = serde_json::from_str(&json).unwrap();
        match back {
            AdapterEvent::PtyChunk { local_id, data } => {
                assert_eq!(local_id, "s1");
                assert_eq!(data, "aGk=");
            }
            other => panic!("expected PtyChunk, got {other:?}"),
        }
    }

    #[test]
    fn session_spec_minimal_roundtrips() {
        let spec = SessionSpec {
            adapter_id: AdapterId::new("claude-code"),
            working_dir: None,
            prompt: None,
            name: None,
            permission_mode: None,
            effort: None,
            model: None,
            service_tier: None,
            env: std::collections::BTreeMap::new(),
            bootstrap: serde_json::Value::Null,
            parent_local_id: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let _back: SessionSpec = serde_json::from_str(&json).unwrap();
    }
}
