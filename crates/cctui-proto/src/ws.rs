use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::adapter::{AdapterCommand, AdapterEvent, BootstrapFile};
use crate::api::DaemonAdapterConfig;

// --- Daemon → Server ---

/// Exactly one of `data` (base64, up to [`READ_FILE_INLINE_BYTES`]) or
/// `blob_hash` is set. `sha256` is the content hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadFileOk {
    pub name: String,
    pub size: u64,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blob_hash: Option<String>,
}

/// Maps to HTTP 403 / 413 / 404 / 500.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadFileErrorKind {
    Denied,
    TooLarge,
    NotFound,
    Io,
}

/// Larger files go through the blob store.
pub const READ_FILE_INLINE_BYTES: u64 = 1024 * 1024;

pub const READ_FILE_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Frames sent by a daemon to the server over `/api/v1/daemon/ws`.
///
/// `Event` is inherently the largest variant (it carries an [`AdapterEvent`]
/// with JSON payloads / many optional fields); boxing it would ripple
/// through every construct/match site for no real benefit on this
/// non-hot-path wire enum.

/// Daemon → server frames on `/api/v1/daemon/ws`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
#[allow(clippy::large_enum_variant)]
pub enum DaemonFrameUp {
    Event {
        adapter_id: String,
        event: AdapterEvent,
    },
    /// Registration hint for when `SessionStarted` is not available yet.
    SessionRegistered {
        adapter_id: String,
        local_id: String,
    },
    /// Liveness ping. Optional fields are omitted by daemons that lack them.
    Heartbeat {
        sent_at: chrono::DateTime<chrono::Utc>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bandwidth: Option<crate::bandwidth::BandwidthSummary>,
        /// `None` leaves the stored flag unchanged.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        update_hook: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resources: Option<crate::resources::MachineResources>,
        /// Job shorts on disk, answered by [`DaemonFrameDown::ArchivedJobs`].
        #[serde(default, skip_serializing_if = "Option::is_none")]
        claude_jobs: Option<Vec<String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        harness: Option<crate::harness::HarnessReport>,
    },
    /// Reply to [`DaemonFrameDown::StageFiles`].
    StageFilesResult {
        request_id: uuid::Uuid,
        ok: bool,
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Reply to [`DaemonFrameDown::ListDirs`].
    ListDirsResult {
        request_id: uuid::Uuid,
        ok: bool,
        #[serde(default)]
        dirs: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Reply to [`DaemonFrameDown::GitInfo`].
    GitInfoResult {
        request_id: uuid::Uuid,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        info: Option<crate::git::GitInfo>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// Reply to [`DaemonFrameDown::ReadFile`].
    ReadFileResult {
        request_id: uuid::Uuid,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<ReadFileOk>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error_kind: Option<ReadFileErrorKind>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// One chunk of an up-frame split by [`crate::chunk`]. `transfer_id` is the
    /// payload hash; `data` is base64. `codec: Some("zstd")` means the joined
    /// payload must be decompressed before parsing.
    Chunk {
        transfer_id: String,
        chunk_index: u32,
        total_chunks: u32,
        data: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        codec: Option<String>,
    },
    /// A compressed up-frame small enough not to need chunking.
    Compressed {
        codec: String,
        data: String,
    },
    /// Frames coalesced for better compression, processed in order.
    Batch {
        frames: Vec<Self>,
    },
}

/// Server → daemon frames on `/api/v1/daemon/ws`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DaemonFrameDown {
    /// Declarative state, sent on connect and on every change.
    Reconcile {
        adapters: Vec<DaemonAdapterConfig>,
        #[serde(default)]
        secret_scrub: SecretScrubConfig,
    },
    Command {
        adapter_id: String,
        command: Box<AdapterCommand>,
    },
    /// Event `seq` is durably stored; the daemon may trim its spool.
    Ack {
        seq: u64,
    },
    /// Stage mid-chat attachments; answered by [`DaemonFrameUp::StageFilesResult`].
    StageFiles {
        request_id: uuid::Uuid,
        adapter_id: String,
        local_id: String,
        uploads: Vec<BootstrapFile>,
    },
    /// One level of sub-directories of `path` (`~` expanded).
    ListDirs {
        request_id: uuid::Uuid,
        path: String,
    },
    /// Git facts for `path`. `include_dirty` runs `git status`.
    GitInfo {
        request_id: uuid::Uuid,
        path: String,
        #[serde(default)]
        include_dirty: bool,
    },
    /// Read one file within the daemon's allow-list (temp dirs, `$HOME`, `cwd`).
    /// Files over `max_bytes` fail with [`ReadFileErrorKind::TooLarge`].
    ReadFile {
        request_id: uuid::Uuid,
        path: String,
        max_bytes: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<String>,
    },
    /// `None` means no usable prefix; restart from chunk 0.
    ChunkAck {
        transfer_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        highest_contiguous_chunk: Option<u32>,
    },
    /// `local_id` → stored transcript byte offset, sent right after `Reconcile`.
    ResumeMarks {
        session_marks: Vec<(String, u64)>,
        /// Superseded by [`Self::ArchivedJobs`].
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        archived: Vec<String>,
    },
    /// Archived sessions among the reported `claude_jobs`; the daemon removes
    /// their jobs. Sent only to daemons that report `claude_jobs`.
    ArchivedJobs {
        session_ids: Vec<String>,
    },
    /// Re-run codex `model/list`; the result arrives as
    /// [`AdapterEvent::CodexModels`](crate::adapter::AdapterEvent::CodexModels).
    RefreshCodexModels {},
    /// Run the update hook for `version`. No reply: the hook restarts the server,
    /// so progress is posted to `/api/v1/daemon/update-hook/{run_id}`.
    RunUpdateHook {
        run_id: uuid::Uuid,
        version: String,
        release_url: String,
    },
    /// Sent only to daemons that report `harness`.
    HarnessUpdatePolicy {
        policy: crate::harness::HarnessUpdatePolicy,
    },
}

/// User patterns only; compiled defaults live in `cctui-crypto`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecretScrubConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub patterns: Vec<ScrubPattern>,
}

/// Validated server-side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrubPattern {
    pub name: String,
    pub regex: String,
}

// --- Dispatcher ↔ Server ---

/// Dispatch intent sent to an enrolled dispatcher. `payload` is forwarded verbatim.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireDispatchSpec {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_minutes: Option<u32>,
    /// Bearer capability; never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_url: Option<String>,
    /// Derives the worker job name. `None` = `session_id`, i.e. no dedup.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedup_key: Option<String>,
    /// Operator-authored profile name. `None` = `payload.profile`, then the default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub payload: serde_json::Value,
}

/// Server → dispatcher frames on `/api/v1/dispatcher/ws`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DispatcherFrameDown {
    Dispatch { request_id: uuid::Uuid, spec: WireDispatchSpec },
    Status { request_id: uuid::Uuid, handle: String },
    Cancel { request_id: uuid::Uuid, handle: String },
}

/// Dispatcher → server frames on `/api/v1/dispatcher/ws`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[non_exhaustive]
pub enum DispatcherFrameUp {
    Hello {
        kind: String,
        version: String,
    },
    Heartbeat {
        sent_at: chrono::DateTime<chrono::Utc>,
    },
    /// `status` is `dispatched` / `deduplicated` / `redispatched`.
    DispatchResult {
        request_id: uuid::Uuid,
        session_id: String,
        handle: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        namespace: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    /// `state` is `running` / `complete` / `failed` / `gone`.
    StatusResult {
        request_id: uuid::Uuid,
        handle: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        state: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
    CancelResult {
        request_id: uuid::Uuid,
        handle: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

// --- Agent → Server (stream events) ---

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    /// `meta` marks text injected into the agent rather than typed by the human.
    /// `seq` is the per-session insert order; use it, not `ts`, for ordering.
    Text {
        content: String,
        #[serde(default)]
        meta: bool,
        /// `thinking` | `redacted_thinking` | `attachment` | `system_marker` |
        /// `turn_annotation` | `queue_op`. `None` is visible prose.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        /// For `queue_op`: `queued` | `dequeued` | `removed` | `cleared`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation: Option<String>,
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<crate::models::TokenUsage>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
        /// `None` for assistant text and turns cctui did not originate.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<uuid::Uuid>,
    },
    ToolCall {
        tool: String,
        input: serde_json::Value,
        /// `server_tool_use` for provider-executed tools.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    ToolResult {
        tool: String,
        output_summary: String,
        /// `server_tool_result` for provider-executed tools.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        kind: Option<String>,
        #[serde(default)]
        error: bool,
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    Heartbeat {
        tokens_in: u64,
        tokens_out: u64,
        cost_usd: f64,
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    Reply {
        content: String,
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<uuid::Uuid>,
    },
    /// `/clear` boundary; the session id rotates under the same worker.
    ContextReset {
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    /// `/compact` boundary with its summary text.
    CompactSummary {
        content: String,
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    /// Post-turn summary, rendered as footer text on the last assistant message.
    TurnSummary {
        detail: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_category: Option<String>,
        #[serde(default)]
        needs_action: bool,
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
    TurnEnd {
        ts: i64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        seq: Option<i64>,
    },
}

impl AgentEvent {
    /// `None` before persistence.
    #[must_use]
    pub const fn seq(&self) -> Option<i64> {
        match self {
            Self::Text { seq, .. }
            | Self::ToolCall { seq, .. }
            | Self::ToolResult { seq, .. }
            | Self::Heartbeat { seq, .. }
            | Self::Reply { seq, .. }
            | Self::ContextReset { seq, .. }
            | Self::CompactSummary { seq, .. }
            | Self::TurnSummary { seq, .. }
            | Self::TurnEnd { seq, .. } => *seq,
        }
    }

    pub const fn set_seq(&mut self, value: i64) {
        let slot = match self {
            Self::Text { seq, .. }
            | Self::ToolCall { seq, .. }
            | Self::ToolResult { seq, .. }
            | Self::Heartbeat { seq, .. }
            | Self::Reply { seq, .. }
            | Self::ContextReset { seq, .. }
            | Self::CompactSummary { seq, .. }
            | Self::TurnSummary { seq, .. }
            | Self::TurnEnd { seq, .. } => seq,
        };
        *slot = Some(value);
    }

    /// No-op for variants other than `Text` and `Reply`.
    pub const fn set_turn_id(&mut self, value: uuid::Uuid) {
        match self {
            Self::Text { turn_id, .. } | Self::Reply { turn_id, .. } => *turn_id = Some(value),
            _ => {}
        }
    }
}

// --- TUI → Server ---

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TuiCommand {
    Subscribe {
        session_id: String,
    },
    Unsubscribe {
        session_id: String,
    },
    /// The daemon attaches its viewer PTY only while at least one client watches.
    WatchTerminal {
        session_id: String,
        watch: bool,
    },
    /// `client_msg_id` requests a [`ServerEvent::MessageAck`].
    Message {
        session_id: String,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        client_msg_id: Option<String>,
        /// 0-based option picks per question; `content` stays the text fallback.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ask_picks: Option<Vec<Vec<usize>>>,
        /// Client-minted UUIDv7.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        turn_id: Option<uuid::Uuid>,
    },
    PermissionResponse {
        session_id: String,
        request_id: String,
        behavior: String,
    },
}

// --- Server → TUI ---

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerEvent {
    Stream {
        session_id: String,
        data: AgentEvent,
    },
    Status {
        session_id: String,
        status: crate::models::SessionStatus,
    },
    SessionRegistered {
        session: crate::models::Session,
    },
    SessionDeregistered {
        session_id: String,
    },
    PermissionRequest {
        session_id: String,
        request_id: String,
        tool_name: String,
        description: String,
        input_preview: String,
    },
    PermissionResolved {
        session_id: String,
        request_id: String,
    },
    AskQuestion {
        session_id: String,
        question: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        questions: Option<serde_json::Value>,
        /// Assistant text preceding the question in the same turn.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preamble: Option<String>,
    },
    AskResolved {
        session_id: String,
    },
    PlanRequest {
        session_id: String,
        plan: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        preamble: Option<String>,
    },
    PlanResolved {
        session_id: String,
    },
    /// Outcome of a client-initiated command.
    CommandResult {
        command_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Scopes delivery to the session owner.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
    SessionEnded {
        session_id: String,
        reason: crate::models::SessionEndReason,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
    },
    /// Sent only to the originating socket. `ok` means queued to a daemon, not delivered.
    MessageAck {
        session_id: String,
        client_msg_id: String,
        ok: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Delivery is confirmed by [`ServerEvent::CommandResult`] under this id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command_id: Option<uuid::Uuid>,
    },
    ArchiveManifest {
        machine_id: uuid::Uuid,
        count: i64,
    },
    /// Sent on tier transitions.
    MachineLiveness {
        machine_id: uuid::Uuid,
        liveness: crate::models::MachineLiveness,
    },
    MachineResources {
        machine_id: uuid::Uuid,
        resources: crate::resources::MachineResources,
    },
    /// `usage` is the row the accounts usage routes return.
    AccountUsage {
        account_id: uuid::Uuid,
        usage: serde_json::Value,
    },
    DispatcherLiveness {
        dispatcher_id: uuid::Uuid,
        liveness: crate::models::MachineLiveness,
    },
    ArchiveUploaded {
        machine_id: uuid::Uuid,
        project_dir: String,
        session_id: String,
        size_bytes: i64,
        sha256: String,
    },
    /// Synced GitHub state changed; clients refetch the located rows over HTTP.
    GithubEvent {
        kind: crate::github::GithubEventKind,
        payload: crate::github::GithubEventPayload,
    },
    /// The per-account soft limit refused this session's request (429).
    SoftLimitReached {
        session_id: String,
        account_id: uuid::Uuid,
        account_name: String,
        reason: String,
        retry_after_secs: i64,
    },
    SoftLimitCleared {
        session_id: String,
    },
    /// The account's tool-call policy blocked a tool call. `rule` holds the rule
    /// name and a masked match; raw input is never carried.
    ToolCallBlocked {
        session_id: String,
        tool_name: String,
        rule: String,
    },
    /// Base64 PTY bytes. Not persisted.
    PtyChunk {
        session_id: String,
        data: String,
    },
    /// Application-level liveness tick; browsers cannot observe WS pings.
    Heartbeat {},
    /// This socket lagged. Refetch the session, or everything when `None`.
    /// Never broadcast.
    Resync {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_event_tagged_serialization() {
        let event = AgentEvent::Text {
            content: "hello".into(),
            meta: false,
            kind: None,
            operation: None,
            ts: 1_234_567_890,
            message_id: None,
            usage: None,
            seq: None,
            turn_id: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"text""#));
        assert!(json.contains(r#""content":"hello""#));
    }

    #[test]
    fn agent_event_seq_roundtrips_and_defaults_to_none() {
        // Legacy payload (no seq field) decodes as None.
        let legacy = r#"{"type":"turn_end","ts":5}"#;
        let ev: AgentEvent = serde_json::from_str(legacy).unwrap();
        assert_eq!(ev.seq(), None);

        // A stamped seq survives a wire roundtrip and is readable via `seq()`.
        let mut ev = AgentEvent::Text {
            content: "hi".into(),
            meta: false,
            kind: None,
            operation: None,
            ts: 10,
            message_id: None,
            usage: None,
            seq: None,
            turn_id: None,
        };
        ev.set_seq(42);
        assert_eq!(ev.seq(), Some(42));
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""seq":42"#));
        let back: AgentEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(back.seq(), Some(42));
    }

    #[test]
    fn agent_event_seq_orders_ask_turn_when_ts_ties_or_inverts() {
        // Reload scenario: a late-flushed AskUserQuestion card+preamble
        // carry a `ts` at/after the user's answer, but their insert `seq` is
        // lower. Ordering by `seq` restores causal order; ordering by `ts` does
        // not. `seq` is the DB insert sequence, always a strict total order.
        let preamble = AgentEvent::Text {
            content: "Here is my analysis.".into(),
            meta: false,
            kind: None,
            operation: None,
            ts: 100, // ties the answer's ts
            message_id: None,
            usage: None,
            seq: Some(1),
            turn_id: None,
        };
        let card = AgentEvent::ToolCall {
            tool: "AskUserQuestion".into(),
            input: serde_json::json!({}),
            kind: None,
            ts: 100, // ties, and flushed late
            seq: Some(2),
        };
        let answer = AgentEvent::Text {
            content: "▷ User: option A".into(),
            meta: false,
            kind: None,
            operation: None,
            ts: 100,
            message_id: None,
            usage: None,
            seq: Some(3),
            turn_id: None,
        };
        // Deliberately shuffled so a stable ts-only sort would leave the answer
        // ahead of its own question.
        let mut events = [answer, preamble, card];
        events.sort_by_key(super::AgentEvent::seq);
        let seqs: Vec<Option<i64>> = events.iter().map(AgentEvent::seq).collect();
        assert_eq!(seqs, vec![Some(1), Some(2), Some(3)]);
        assert!(matches!(&events[2], AgentEvent::Text { content, .. } if content.contains("User")));
    }

    #[test]
    fn tui_command_tagged_serialization() {
        let cmd = TuiCommand::Subscribe { session_id: "test-session".into() };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"subscribe""#));
    }

    #[test]
    fn agent_event_reply_serialization() {
        let event =
            AgentEvent::Reply { content: "acknowledged".into(), ts: 100, seq: None, turn_id: None };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"reply""#));
        assert!(json.contains(r#""content":"acknowledged""#));
    }

    #[test]
    fn agent_event_tool_call_serialization() {
        let event = AgentEvent::ToolCall {
            tool: "Bash".into(),
            input: serde_json::json!({"command": "ls"}),
            kind: None,
            ts: 42,
            seq: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_call""#));
        assert!(json.contains(r#""tool":"Bash""#));
    }

    #[test]
    fn agent_event_tool_result_serialization() {
        let event = AgentEvent::ToolResult {
            tool: "Bash".into(),
            output_summary: "file.txt".into(),
            kind: None,
            error: false,
            ts: 42,
            seq: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"tool_result""#));
        assert!(json.contains(r#""output_summary":"file.txt""#));
    }

    #[test]
    fn agent_event_heartbeat_serialization() {
        let event = AgentEvent::Heartbeat {
            tokens_in: 100,
            tokens_out: 50,
            cost_usd: 0.01,
            ts: 42,
            seq: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"heartbeat""#));
        assert!(json.contains(r#""tokens_in":100"#));
    }

    #[test]
    fn agent_event_roundtrip_all_variants() {
        let variants = vec![
            AgentEvent::Text {
                content: "hello".into(),
                meta: false,
                kind: None,
                operation: None,
                ts: 1,
                message_id: None,
                usage: None,
                seq: None,
                turn_id: None,
            },
            AgentEvent::ToolCall {
                tool: "Read".into(),
                input: serde_json::json!({}),
                kind: None,
                ts: 2,
                seq: None,
            },
            AgentEvent::ToolResult {
                tool: "Read".into(),
                output_summary: "ok".into(),
                kind: Some("server_tool_result".into()),
                error: true,
                ts: 3,
                seq: None,
            },
            AgentEvent::Heartbeat {
                tokens_in: 10,
                tokens_out: 5,
                cost_usd: 0.001,
                ts: 4,
                seq: None,
            },
            AgentEvent::Reply { content: "done".into(), ts: 5, seq: None, turn_id: None },
            AgentEvent::TurnEnd { ts: 6, seq: None },
        ];
        for event in variants {
            let json = serde_json::to_string(&event).unwrap();
            let deserialized: AgentEvent = serde_json::from_str(&json).unwrap();
            let re_json = serde_json::to_string(&deserialized).unwrap();
            assert_eq!(json, re_json, "roundtrip failed for {json}");
        }
    }

    #[test]
    fn server_event_serialization() {
        let event = ServerEvent::Stream {
            session_id: "test-session".into(),
            data: AgentEvent::Text {
                content: "hi".into(),
                meta: false,
                kind: None,
                operation: None,
                ts: 1,
                message_id: None,
                usage: None,
                seq: None,
                turn_id: None,
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains(r#""type":"stream""#));
    }

    #[test]
    fn daemon_frame_up_event_serializes_tagged() {
        let f = DaemonFrameUp::Event {
            adapter_id: "claude-code".into(),
            event: AdapterEvent::SessionStarted {
                local_id: "abc".into(),
                meta: crate::adapter::SessionMeta::default(),
            },
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"event""#));
        assert!(json.contains(r#""adapter_id":"claude-code""#));
        let _back: DaemonFrameUp = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn heartbeat_carries_bandwidth_and_accepts_legacy_payload() {
        let hb = DaemonFrameUp::Heartbeat {
            sent_at: chrono::Utc::now(),
            bandwidth: Some(crate::bandwidth::BandwidthSummary {
                forward: 900,
                blob_put: 42,
                ..Default::default()
            }),
            update_hook: Some(true),
            resources: Some(crate::resources::MachineResources {
                cpu_pct: 12.5,
                ..Default::default()
            }),
            claude_jobs: Some(vec!["deadbeef".into()]),
            harness: Some(crate::harness::HarnessReport::default()),
        };
        let json = serde_json::to_string(&hb).unwrap();
        assert!(json.contains(r#""forward":900"#), "{json}");
        assert!(json.contains(r#""claude_jobs":["deadbeef"]"#), "{json}");
        assert!(json.contains(r#""cpu_pct":12.5"#), "{json}");
        assert!(json.contains(r#""blob_put":42"#), "{json}");
        assert!(json.contains(r#""update_hook":true"#), "{json}");

        let legacy = r#"{"type":"heartbeat","sent_at":"2026-07-21T00:00:00Z"}"#;
        let back: DaemonFrameUp = serde_json::from_str(legacy).unwrap();
        match back {
            // A daemon that predates either field says nothing about both; the
            // server must not read that silence as "no hook".
            DaemonFrameUp::Heartbeat {
                bandwidth,
                update_hook,
                resources,
                claude_jobs,
                harness,
                ..
            } => {
                assert!(harness.is_none());
                assert!(bandwidth.is_none());
                assert!(update_hook.is_none());
                assert!(resources.is_none());
                assert!(claude_jobs.is_none());
            }
            _ => panic!("expected Heartbeat"),
        }
    }

    #[test]
    fn run_update_hook_roundtrips() {
        let f = DaemonFrameDown::RunUpdateHook {
            run_id: uuid::Uuid::nil(),
            version: "0.7.319".into(),
            release_url: "https://github.com/DorskFR/cctui/releases/tag/v0.7.319".into(),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"run_update_hook""#), "{json}");
        let back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
        match back {
            DaemonFrameDown::RunUpdateHook { version, .. } => assert_eq!(version, "0.7.319"),
            _ => panic!("expected RunUpdateHook"),
        }
    }

    #[test]
    fn harness_update_policy_roundtrips() {
        let f = DaemonFrameDown::HarnessUpdatePolicy {
            policy: crate::harness::HarnessUpdatePolicy { enabled: true, ..Default::default() },
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"harness_update_policy""#), "{json}");
        match serde_json::from_str::<DaemonFrameDown>(&json).unwrap() {
            DaemonFrameDown::HarnessUpdatePolicy { policy } => assert!(policy.enabled),
            _ => panic!("expected HarnessUpdatePolicy"),
        }
    }

    #[test]
    fn daemon_frame_down_reconcile_roundtrips() {
        let f = DaemonFrameDown::Reconcile {
            adapters: vec![],
            secret_scrub: SecretScrubConfig::default(),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"reconcile""#));
        let _back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn daemon_frame_down_command_roundtrips() {
        let f = DaemonFrameDown::Command {
            adapter_id: "claude-code".into(),
            command: Box::new(AdapterCommand::Kill { local_id: "abc".into(), signal: None }),
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"command""#));
        let _back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn daemon_frame_down_resume_marks_roundtrips() {
        let f = DaemonFrameDown::ResumeMarks {
            session_marks: vec![("sess-1".into(), 4096), ("sess-2".into(), 0)],
            archived: vec!["sess-3".into()],
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"resume_marks""#));
        let back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
        match back {
            DaemonFrameDown::ResumeMarks { session_marks, archived } => {
                assert_eq!(session_marks, vec![("sess-1".into(), 4096), ("sess-2".into(), 0)]);
                assert_eq!(archived, vec!["sess-3".to_string()]);
            }
            _ => panic!("expected ResumeMarks"),
        }
    }

    #[test]
    fn daemon_frame_down_archived_jobs_roundtrips() {
        let f = DaemonFrameDown::ArchivedJobs { session_ids: vec!["sess-3".into()] };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"archived_jobs""#));
        match serde_json::from_str::<DaemonFrameDown>(&json).unwrap() {
            DaemonFrameDown::ArchivedJobs { session_ids } => {
                assert_eq!(session_ids, vec!["sess-3".to_string()]);
            }
            _ => panic!("expected ArchivedJobs"),
        }
    }

    #[test]
    fn tui_command_message_serialization() {
        let cmd = TuiCommand::Message {
            session_id: "test-session".into(),
            content: "hello".into(),
            client_msg_id: None,
            ask_picks: None,
            turn_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"message""#));
        assert!(json.contains(r#""content":"hello""#));
        let deserialized: TuiCommand = serde_json::from_str(&json).unwrap();
        match deserialized {
            TuiCommand::Message { content, .. } => assert_eq!(content, "hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn tui_command_message_omits_client_msg_id_when_none() {
        // Old clients send no `client_msg_id`; the field is skipped on the wire
        // so the payload stays byte-compatible with readers.
        let cmd = TuiCommand::Message {
            session_id: "s".into(),
            content: "hi".into(),
            client_msg_id: None,
            ask_picks: None,
            turn_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(!json.contains("client_msg_id"), "None must be skipped: {json}");
    }

    #[test]
    fn tui_command_message_accepts_legacy_payload_without_client_msg_id() {
        // A frame from an older client (no field) must still decode (serde default).
        let legacy = r#"{"type":"message","session_id":"s","content":"hi"}"#;
        let cmd: TuiCommand = serde_json::from_str(legacy).unwrap();
        match cmd {
            TuiCommand::Message { client_msg_id, .. } => assert_eq!(client_msg_id, None),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn tui_command_message_carries_client_msg_id_when_set() {
        let cmd = TuiCommand::Message {
            session_id: "s".into(),
            content: "hi".into(),
            client_msg_id: Some("abc-123".into()),
            ask_picks: None,
            turn_id: None,
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""client_msg_id":"abc-123""#));
        let back: TuiCommand = serde_json::from_str(&json).unwrap();
        match back {
            TuiCommand::Message { client_msg_id, .. } => {
                assert_eq!(client_msg_id.as_deref(), Some("abc-123"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn server_event_message_ack_roundtrips() {
        let ev = ServerEvent::MessageAck {
            session_id: "s".into(),
            client_msg_id: "abc-123".into(),
            ok: false,
            error: Some("no daemon connected for machine …".into()),
            command_id: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""type":"message_ack""#));
        assert!(json.contains(r#""ok":false"#));
        assert!(json.contains(r#""client_msg_id":"abc-123""#));
        let _back: ServerEvent = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn dispatcher_frame_down_dispatch_roundtrips() {
        let f = DispatcherFrameDown::Dispatch {
            request_id: uuid::Uuid::nil(),
            spec: WireDispatchSpec {
                session_id: "sess-1".into(),
                timeout_minutes: Some(30),
                reply_url: None,
                dedup_key: None,
                profile: None,
                payload: serde_json::json!({"name": "demo"}),
            },
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"dispatch""#));
        assert!(!json.contains("reply_url"), "None reply_url must be skipped: {json}");
        let _back: DispatcherFrameDown = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn dispatcher_frame_up_roundtrips() {
        let frames = vec![
            DispatcherFrameUp::Hello { kind: "docker".into(), version: "0.0.0".into() },
            DispatcherFrameUp::Heartbeat { sent_at: chrono::Utc::now() },
            DispatcherFrameUp::DispatchResult {
                request_id: uuid::Uuid::nil(),
                session_id: "sess-1".into(),
                handle: "container/cctui-worker-abc".into(),
                namespace: None,
                status: Some("dispatched".into()),
                error: None,
            },
            DispatcherFrameUp::StatusResult {
                request_id: uuid::Uuid::nil(),
                handle: "container/cctui-worker-abc".into(),
                state: Some("running".into()),
                error: None,
            },
            DispatcherFrameUp::CancelResult {
                request_id: uuid::Uuid::nil(),
                handle: "container/cctui-worker-abc".into(),
                ok: true,
                error: None,
            },
        ];
        for f in frames {
            let json = serde_json::to_string(&f).unwrap();
            let _back: DispatcherFrameUp = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn daemon_frame_down_list_dirs_roundtrips() {
        let f = DaemonFrameDown::ListDirs { request_id: uuid::Uuid::nil(), path: "/home".into() };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"list_dirs""#));
        let _back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn daemon_frame_down_refresh_codex_models_roundtrips() {
        let json = serde_json::to_string(&DaemonFrameDown::RefreshCodexModels {}).unwrap();
        assert!(json.contains(r#""type":"refresh_codex_models""#));
        let back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, DaemonFrameDown::RefreshCodexModels {}));
    }

    #[test]
    fn daemon_frame_up_list_dirs_result_roundtrips() {
        let f = DaemonFrameUp::ListDirsResult {
            request_id: uuid::Uuid::nil(),
            ok: true,
            dirs: vec!["projects".into()],
            error: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains(r#""type":"list_dirs_result""#));
        assert!(!json.contains("error"), "None error must be skipped: {json}");
        let _back: DaemonFrameUp = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn daemon_read_file_frames_roundtrip() {
        let down = DaemonFrameDown::ReadFile {
            request_id: uuid::Uuid::nil(),
            path: "~/out/report.md".into(),
            max_bytes: READ_FILE_MAX_BYTES,
            cwd: Some("/home/u/proj".into()),
        };
        let json = serde_json::to_string(&down).unwrap();
        assert!(json.contains(r#""type":"read_file""#));
        let _back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
        let legacy = r#"{"type":"read_file","request_id":"00000000-0000-0000-0000-000000000000","path":"/x","max_bytes":1}"#;
        let back: DaemonFrameDown = serde_json::from_str(legacy).unwrap();
        assert!(matches!(back, DaemonFrameDown::ReadFile { cwd: None, .. }));

        let up = DaemonFrameUp::ReadFileResult {
            request_id: uuid::Uuid::nil(),
            ok: true,
            file: Some(ReadFileOk {
                name: "report.md".into(),
                size: 3,
                sha256: "ab".into(),
                media_type: Some("text/markdown".into()),
                data: Some("YWJj".into()),
                blob_hash: None,
            }),
            error_kind: None,
            error: None,
        };
        let json = serde_json::to_string(&up).unwrap();
        assert!(json.contains(r#""type":"read_file_result""#));
        assert!(!json.contains("blob_hash"), "None fields must be skipped: {json}");
        let _back: DaemonFrameUp = serde_json::from_str(&json).unwrap();

        let err = DaemonFrameUp::ReadFileResult {
            request_id: uuid::Uuid::nil(),
            ok: false,
            file: None,
            error_kind: Some(ReadFileErrorKind::TooLarge),
            error: Some("too big".into()),
        };
        let json = serde_json::to_string(&err).unwrap();
        assert!(json.contains(r#""error_kind":"too_large""#));
        let _back: DaemonFrameUp = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn daemon_git_info_frames_roundtrip() {
        let down = DaemonFrameDown::GitInfo {
            request_id: uuid::Uuid::nil(),
            path: "~/repo".into(),
            include_dirty: false,
        };
        let json = serde_json::to_string(&down).unwrap();
        assert!(json.contains(r#""type":"git_info""#));
        let _back: DaemonFrameDown = serde_json::from_str(&json).unwrap();
        // Legacy senders omit include_dirty.
        let legacy = r#"{"type":"git_info","request_id":"00000000-0000-0000-0000-000000000000","path":"/x"}"#;
        let back: DaemonFrameDown = serde_json::from_str(legacy).unwrap();
        assert!(matches!(back, DaemonFrameDown::GitInfo { include_dirty: false, .. }));

        let up = DaemonFrameUp::GitInfoResult {
            request_id: uuid::Uuid::nil(),
            ok: true,
            info: Some(crate::git::GitInfo {
                is_repo: true,
                branch: Some("main".into()),
                ..Default::default()
            }),
            error: None,
        };
        let json = serde_json::to_string(&up).unwrap();
        assert!(json.contains(r#""type":"git_info_result""#));
        assert!(!json.contains("detached_sha"), "None fields must be skipped: {json}");
        let _back: DaemonFrameUp = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn watch_terminal_command_and_pty_chunk_event_roundtrip() {
        let cmd = TuiCommand::WatchTerminal { session_id: "s1".into(), watch: true };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"watch_terminal""#));
        assert!(json.contains(r#""watch":true"#));
        let _back: TuiCommand = serde_json::from_str(&json).unwrap();

        let ev = ServerEvent::PtyChunk { session_id: "s1".into(), data: "aGk=".into() };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains(r#""type":"pty_chunk""#));
        assert!(json.contains(r#""data":"aGk=""#));
        let _back: ServerEvent = serde_json::from_str(&json).unwrap();
    }

    #[test]
    fn server_event_message_ack_omits_error_when_ok() {
        let ev = ServerEvent::MessageAck {
            session_id: "s".into(),
            client_msg_id: "abc-123".into(),
            ok: true,
            error: None,
            command_id: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(!json.contains("error"), "None error must be skipped: {json}");
    }

    #[test]
    fn server_event_heartbeat_serializes_with_type_tag() {
        let json = serde_json::to_string(&ServerEvent::Heartbeat {}).unwrap();
        assert_eq!(json, r#"{"type":"heartbeat"}"#);
        let back: ServerEvent = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, ServerEvent::Heartbeat {}));
    }

    #[test]
    fn server_event_resync_serializes_with_optional_session() {
        let all = serde_json::to_string(&ServerEvent::Resync { session_id: None }).unwrap();
        assert_eq!(all, r#"{"type":"resync"}"#);
        let one =
            serde_json::to_string(&ServerEvent::Resync { session_id: Some("s".into()) }).unwrap();
        assert_eq!(one, r#"{"type":"resync","session_id":"s"}"#);
        let back: ServerEvent = serde_json::from_str(&one).unwrap();
        assert!(matches!(back, ServerEvent::Resync { session_id: Some(s) } if s == "s"));
    }
}
