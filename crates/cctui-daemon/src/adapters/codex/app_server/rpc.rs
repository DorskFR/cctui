use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use cctui_proto::adapter::AdapterEvent;
use serde_json::{Value, json};
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use uuid::Uuid;

use super::diagnose::DiagnoseRings;
use super::notifications::map_notification;

/// Outbound request id seeds. The handshake uses fixed ids so the driver
/// can recognise the responses it is waiting for; everything after is
/// monotonic from [`Self::RUN_BASE`].
pub(super) const ID_INITIALIZE: i64 = 1;
pub(super) const ID_THREAD_START: i64 = 2;
pub(super) const RUN_BASE: i64 = 100;

pub(super) const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// `CommandResult` error for an `Interrupt` received while no turn is active.
pub const NO_TURN_IN_FLIGHT: &str = "no turn in flight";

/// Budget for the whole handshake (`initialize` → model check →
/// `thread/start|resume|fork`), measured from process launch. A resume of a
/// long transcript needs more than one RPC deadline; the spawn caller
/// (webui `awaitCommand`) waits this long plus a margin, so a hung handshake
/// must fail here first.
pub(super) const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(45);

/// Which `decision` vocabulary an approval reply must use. Codex uses two
/// distinct enums depending on the approval method (verified against the
/// app-server JSON schema, codex-cli 0.134):
///
/// - command-execution and file-change approvals →
///   `"accept"` / `"decline"`.
/// - apply-patch and exec-command approvals (legacy `ReviewDecision`) →
///   `"approved"` / `"denied"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalKind {
    AcceptDecline,
    ApprovedDenied,
}

impl ApprovalKind {
    const fn decision(self, allow: bool) -> &'static str {
        match (self, allow) {
            (Self::AcceptDecline, true) => "accept",
            (Self::AcceptDecline, false) => "decline",
            (Self::ApprovedDenied, true) => "approved",
            (Self::ApprovedDenied, false) => "denied",
        }
    }
}

/// Classification of a single inbound JSON-RPC object.
#[derive(Debug)]
pub enum Incoming {
    /// Reply to one of our requests: has `id`, no `method`.
    Response { id: i64, value: Value },
    /// Server→client request that blocks on a decision (tool/patch
    /// approval): carries both `method` and `id`. `rpc_id` is echoed back
    /// verbatim in the reply; `request_id` is the stable id surfaced to the
    /// TUI via [`AdapterEvent::PermissionRequest`].
    Approval { rpc_id: Value, request_id: String, tool: String, kind: ApprovalKind, input: Value },
    /// `item/tool/requestUserInput`: codex's `AskUserQuestion`.
    /// `question_ids` are needed to key the [`ToolRequestUserInputResponse`].
    Question { rpc_id: Value, question: String, questions: Value, question_ids: Vec<String> },
    /// A server→client request cctui cannot fulfil. It carries an `id`, so
    /// leaving it unanswered blocks codex forever; `reply` is the decline/error
    /// to write back immediately instead.
    Decline { reply: Value },
    /// A notification we mapped onto an adapter event.
    Event(AdapterEvent),
    /// A schema-known notification with no user signal of its own. `reason`
    /// records why nothing is emitted; the frame stays in the diagnose ring.
    Traced { method: String, reason: &'static str },
    /// A method absent from the pinned protocol schema — a newer codex added
    /// it. Traced loudly so a protocol addition surfaces as a gap, and still
    /// carried into the timeline.
    Unhandled { method: String, event: AdapterEvent },
    /// A frame that is neither request, response nor notification.
    Ignored,
}

/// Classify one parsed JSON-RPC object. `local_id` is the thread/session id
/// (only meaningful once the handshake has completed; during the handshake
/// only [`Incoming::Response`] values are acted upon).
#[must_use]
pub fn classify(local_id: &str, v: &Value) -> Incoming {
    let has_id = v.get("id").is_some();
    let method = v.get("method").and_then(Value::as_str);
    match (method, has_id) {
        (Some(m), true) => classify_server_request(m, v),
        (Some(m), false) => map_notification(local_id, m, v),
        (None, true) => {
            let id = v.get("id").and_then(Value::as_i64).unwrap_or(-1);
            Incoming::Response { id, value: v.clone() }
        }
        (None, false) => Incoming::Ignored,
    }
}

fn classify_server_request(method: &str, v: &Value) -> Incoming {
    let params = v.get("params").cloned().unwrap_or(Value::Null);
    let rpc_id = v.get("id").cloned().unwrap_or(Value::Null);
    let (kind, tool) = match method {
        "item/commandExecution/requestApproval" => (ApprovalKind::AcceptDecline, "shell"),
        "item/fileChange/requestApproval" => (ApprovalKind::AcceptDecline, "file_change"),
        "applyPatchApproval" => (ApprovalKind::ApprovedDenied, "apply_patch"),
        "execCommandApproval" => (ApprovalKind::ApprovedDenied, "shell"),
        "item/tool/requestUserInput" => return classify_user_input(rpc_id, &params),
        // Known-but-unsupported requests get their schema-correct decline reply
        // so codex isn't blocked forever; everything else (dynamic
        // tool call, token refresh, attestation, future methods) gets a generic
        // method-not-supported error.
        "mcpServer/elicitation/request" => {
            return Incoming::Decline { reply: elicitation_decline(&rpc_id) };
        }
        "item/permissions/requestApproval" => {
            return Incoming::Decline { reply: permissions_decline(&rpc_id) };
        }
        _ => return Incoming::Decline { reply: request_not_supported(&rpc_id, method) },
    };
    let request_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .map_or_else(|| format!("codex-approval-{rpc_id}"), std::string::ToString::to_string);
    Incoming::Approval { rpc_id, request_id, tool: tool.to_string(), kind, input: params }
}

/// Classify an `item/tool/requestUserInput` request into a [`Incoming::Question`].
/// Flattens the per-question `header`/`question` into a single text
/// (for the flattened claude field) while passing the raw `questions` array
/// through for the interactive card, and collects the question ids the answer
/// must be keyed on.
fn classify_user_input(rpc_id: Value, params: &Value) -> Incoming {
    let questions = params.get("questions").cloned().unwrap_or_else(|| json!([]));
    let list = questions.as_array().cloned().unwrap_or_default();
    let question_ids: Vec<String> = list
        .iter()
        .filter_map(|q| q.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect();
    let question = list
        .iter()
        .filter_map(|q| {
            let text = q.get("question").and_then(Value::as_str)?;
            Some(
                q.get("header")
                    .and_then(Value::as_str)
                    .filter(|h| !h.is_empty())
                    .map_or_else(|| text.to_owned(), |header| format!("{header} — {text}")),
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    Incoming::Question { rpc_id, question, questions, question_ids }
}

/// Decline an MCP `elicitation/create` request. cctui does not render
/// the typed form, so it answers `decline` — the schema's neutral "user did not
/// provide input" action — rather than leaving the turn blocked.
fn elicitation_decline(rpc_id: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"action": "decline"}})
}

/// Decline a sandbox-permission elevation request. Granting nothing
/// (an empty `GrantedPermissionProfile`) is the deny: codex continues the turn
/// without the extra permissions instead of waiting on a reply that never comes.
fn permissions_decline(rpc_id: &Value) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"permissions": {}}})
}

/// Reject a server request method cctui does not implement with a JSON-RPC
/// method-not-found error, so codex fails the request fast instead of blocking.
fn request_not_supported(rpc_id: &Value, method: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": rpc_id,
        "error": {"code": -32601, "message": format!("cctui does not support server request {method}")},
    })
}

/// Reply to an `item/tool/requestUserInput` request. The single free
/// text answer is mapped onto every question id — requestUserInput forms are
/// single-question in practice, and codex feeds the string straight to the tool.
pub(super) fn user_input_reply(rpc_id: &Value, question_ids: &[String], answer: &str) -> Value {
    let answers: serde_json::Map<String, Value> =
        question_ids.iter().map(|id| (id.clone(), json!({"answers": [answer]}))).collect();
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"answers": answers}})
}

/// Reply to a server-issued approval request. `rpc_id` must be the exact
/// `id` value from the request; `kind` selects the decision vocabulary.
pub(super) fn approval_reply(rpc_id: &Value, kind: ApprovalKind, allow: bool) -> Value {
    json!({"jsonrpc": "2.0", "id": rpc_id, "result": {"decision": kind.decision(allow)}})
}

/// One outstanding outbound JSON-RPC request.
#[derive(Debug)]
pub struct PendingRpc {
    pub method: String,
    /// Server-minted correlation id: when set, the request's outcome is
    /// reported back as an [`AdapterEvent::CommandResult`].
    pub command_id: Option<Uuid>,
    pub deadline: Instant,
}

impl PendingRpc {
    /// Whether this request is part of the session-establishing handshake —
    /// its failure means the session cannot run at all.
    #[must_use]
    pub fn is_handshake(&self) -> bool {
        matches!(
            self.method.as_str(),
            "initialize" | "thread/start" | "thread/resume" | "thread/fork"
        )
    }
}

/// Correlation table for outbound JSON-RPC requests, keyed by request id.
/// The driver inserts before each write, resolves on the matching
/// response (propagating `error` objects as failures), expires entries past
/// their deadline, and drains everything when the app-server process exits.
#[derive(Debug, Default)]
pub struct PendingRpcs {
    inner: HashMap<i64, PendingRpc>,
}

impl PendingRpcs {
    pub fn insert(&mut self, id: i64, method: &str, command_id: Option<Uuid>, deadline: Instant) {
        self.inner.insert(id, PendingRpc { method: method.to_owned(), command_id, deadline });
    }

    /// Resolve the pending request matching a response `id`. Returns the
    /// entry plus the parsed outcome; `None` for an unknown id.
    pub fn resolve(
        &mut self,
        id: i64,
        response: &Value,
    ) -> Option<(PendingRpc, Result<Value, String>)> {
        let pending = self.inner.remove(&id)?;
        Some((pending, response_outcome(response)))
    }

    /// Forget a request whose write never reached the app-server, so the
    /// retry that re-issues it owns its correlation id alone.
    pub fn remove(&mut self, id: i64) -> Option<PendingRpc> {
        self.inner.remove(&id)
    }

    /// Remove and return every request whose deadline has passed.
    pub fn expire(&mut self, now: Instant) -> Vec<(i64, PendingRpc)> {
        self.inner.extract_if(|_, p| p.deadline <= now).collect()
    }

    /// Remove and return everything — the process is gone, nothing pending
    /// can ever resolve.
    pub fn drain(&mut self) -> Vec<(i64, PendingRpc)> {
        self.inner.drain().collect()
    }

    /// Methods of every outstanding request (diagnostics).
    #[must_use]
    pub fn pending_methods(&self) -> Vec<String> {
        self.inner.values().map(|p| p.method.clone()).collect()
    }

    #[cfg(test)]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// Parse a JSON-RPC response into success (`result`) or failure (the `error`
/// object rendered as a message).
pub(super) fn response_outcome(v: &Value) -> Result<Value, String> {
    let Some(err) = v.get("error").filter(|e| !e.is_null()) else {
        return Ok(v.get("result").cloned().unwrap_or(Value::Null));
    };
    let message = err.get("message").and_then(Value::as_str).unwrap_or("unknown error");
    let mut out = err.get("code").and_then(Value::as_i64).map_or_else(
        || format!("codex app-server error: {message}"),
        |code| format!("codex app-server error {code}: {message}"),
    );
    if let Some(data) = err.get("data").filter(|d| !d.is_null()) {
        use std::fmt::Write as _;
        let _ = write!(out, " ({data})");
    }
    Err(out)
}

pub(super) struct RpcStdin {
    pub(super) inner: RpcSink,
    pub(super) rings: Arc<DiagnoseRings>,
}

pub(super) enum RpcSink {
    Stdio(tokio::process::ChildStdin),
    Shared(crate::adapters::codex::daemon::ThreadSink),
}

impl RpcStdin {
    pub(super) async fn send(&mut self, v: &Value) -> Result<()> {
        self.rings.note_rpc("out", v);
        match &mut self.inner {
            RpcSink::Stdio(stdin) => write_json(stdin, v).await,
            RpcSink::Shared(sink) => sink.send(v).await,
        }
    }
}

/// What the session reads codex's frames from; `Ok(None)` is EOF either way.
pub(super) enum RpcSource {
    Stdio(tokio::io::Lines<BufReader<tokio::process::ChildStdout>>),
    Shared(mpsc::UnboundedReceiver<Value>),
}

impl RpcSource {
    pub(super) async fn next_line(&mut self) -> std::io::Result<Option<String>> {
        match self {
            Self::Stdio(lines) => lines.next_line().await,
            Self::Shared(frames) => Ok(frames.recv().await.map(|v| v.to_string())),
        }
    }
}

pub(super) async fn write_json<W: AsyncWriteExt + Unpin>(w: &mut W, v: &Value) -> Result<()> {
    let mut line = serde_json::to_string(v)?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_response() {
        let v = json!({"id": 2, "result": {"thread": {"sessionId": "abc"}}});
        match classify("", &v) {
            Incoming::Response { id, .. } => assert_eq!(id, 2),
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn classifies_command_approval_request() {
        // Server→client request: has both method and id.
        let v = json!({
            "method": "item/commandExecution/requestApproval",
            "id": 0,
            "params": {"threadId": "t", "itemId": "call_9", "command": "rm -rf /"},
        });
        match classify("sess", &v) {
            Incoming::Approval { rpc_id, request_id, tool, kind, .. } => {
                assert_eq!(rpc_id, json!(0));
                assert_eq!(request_id, "call_9");
                assert_eq!(tool, "shell");
                assert_eq!(kind, ApprovalKind::AcceptDecline);
            }
            other => panic!("expected Approval, got {other:?}"),
        }
    }

    #[test]
    fn classifies_apply_patch_approval() {
        let v = json!({"method": "applyPatchApproval", "id": 5, "params": {"itemId": "p1"}});
        match classify("s", &v) {
            Incoming::Approval { tool, request_id, kind, .. } => {
                assert_eq!(tool, "apply_patch");
                assert_eq!(request_id, "p1");
                assert_eq!(kind, ApprovalKind::ApprovedDenied);
            }
            other => panic!("expected apply_patch Approval, got {other:?}"),
        }
    }

    #[test]
    fn file_change_and_exec_command_approvals_classify() {
        match classify(
            "s",
            &json!({"method": "item/fileChange/requestApproval", "id": 1, "params": {}}),
        ) {
            Incoming::Approval { kind, tool, .. } => {
                assert_eq!(kind, ApprovalKind::AcceptDecline);
                assert_eq!(tool, "file_change");
            }
            other => panic!("expected file-change Approval, got {other:?}"),
        }
        match classify("s", &json!({"method": "execCommandApproval", "id": 1, "params": {}})) {
            Incoming::Approval { kind, .. } => assert_eq!(kind, ApprovalKind::ApprovedDenied),
            other => panic!("expected exec-command Approval, got {other:?}"),
        }
    }

    #[test]
    fn permissions_approval_is_declined_not_left_blocking() {
        // Sandbox-permission elevation has no simple allow/deny reply, so it is
        // declined (empty grant) rather than left hanging.
        let v = json!({"method": "item/permissions/requestApproval", "id": 1, "params": {}});
        match classify("s", &v) {
            Incoming::Decline { reply } => {
                assert_eq!(reply["id"], json!(1));
                assert_eq!(reply["result"]["permissions"], json!({}));
            }
            other => panic!("expected Decline, got {other:?}"),
        }
    }

    #[test]
    fn mcp_elicitation_is_declined() {
        let v = json!({"method": "mcpServer/elicitation/request", "id": 3,
            "params": {"serverName": "s", "threadId": "t", "message": "pick", "mode": "form"}});
        match classify("s", &v) {
            Incoming::Decline { reply } => {
                assert_eq!(reply["id"], json!(3));
                assert_eq!(reply["result"]["action"], "decline");
            }
            other => panic!("expected Decline, got {other:?}"),
        }
    }

    #[test]
    fn unknown_server_request_is_declined_with_error() {
        // A dynamic tool call / future method cctui does not implement must not
        // hang codex: it is answered with a JSON-RPC method-not-found error.
        let v = json!({"method": "item/tool/call", "id": 9, "params": {}});
        match classify("s", &v) {
            Incoming::Decline { reply } => {
                assert_eq!(reply["id"], json!(9));
                assert_eq!(reply["error"]["code"], -32601);
                assert!(reply["error"]["message"].as_str().unwrap().contains("item/tool/call"));
            }
            other => panic!("expected Decline, got {other:?}"),
        }
    }

    #[test]
    fn request_user_input_maps_to_question() {
        let v = json!({"method": "item/tool/requestUserInput", "id": 4, "params": {
        "itemId": "call_42", "threadId": "t", "turnId": "u",
        "questions": [
            {"id": "q1", "header": "Deploy", "question": "Which env?",
             "options": [{"label": "prod", "description": "production"},
                         {"label": "staging", "description": "staging"}]},
        ]}});
        match classify("sess", &v) {
            Incoming::Question { rpc_id, question, questions, question_ids } => {
                assert_eq!(rpc_id, json!(4));
                assert_eq!(question, "Deploy — Which env?");
                assert_eq!(question_ids, vec!["q1".to_owned()]);
                assert_eq!(questions[0]["options"][0]["label"], "prod");
            }
            other => panic!("expected Question, got {other:?}"),
        }
    }

    #[test]
    fn request_user_input_with_no_question_ids_still_maps() {
        let v = json!({"method": "item/tool/requestUserInput", "id": 7,
            "params": {"threadId": "t", "turnId": "u", "questions": []}});
        match classify("s", &v) {
            Incoming::Question { rpc_id, question_ids, .. } => {
                assert_eq!(rpc_id, json!(7));
                assert!(question_ids.is_empty());
            }
            other => panic!("expected Question, got {other:?}"),
        }
    }

    #[test]
    fn user_input_reply_keys_answer_by_question_id() {
        let reply =
            user_input_reply(&json!(4), &["q1".to_owned(), "q2".to_owned()], "prod, us-east");
        assert_eq!(reply["id"], json!(4));
        assert_eq!(reply["result"]["answers"]["q1"]["answers"][0], "prod, us-east");
        assert_eq!(reply["result"]["answers"]["q2"]["answers"][0], "prod, us-east");
    }

    #[test]
    fn decline_reply_builders_shape() {
        assert_eq!(elicitation_decline(&json!(1))["result"]["action"], "decline");
        assert_eq!(permissions_decline(&json!(2))["result"]["permissions"], json!({}));
        assert_eq!(request_not_supported(&json!(3), "x/y")["error"]["code"], -32601);
    }

    #[test]
    fn approval_without_item_id_falls_back_to_rpc_id() {
        let v = json!({"method": "item/commandExecution/requestApproval", "id": 7, "params": {}});
        match classify("s", &v) {
            Incoming::Approval { request_id, .. } => assert_eq!(request_id, "codex-approval-7"),
            other => panic!("expected Approval, got {other:?}"),
        }
    }

    #[test]
    fn approval_reply_uses_correct_decision_vocabulary() {
        // command/file-change family
        let a = approval_reply(&json!(0), ApprovalKind::AcceptDecline, true);
        assert_eq!(a["id"], json!(0));
        assert_eq!(a["result"]["decision"], "accept");
        assert_eq!(
            approval_reply(&json!(0), ApprovalKind::AcceptDecline, false)["result"]["decision"],
            "decline"
        );
        // patch/exec family (ReviewDecision)
        assert_eq!(
            approval_reply(&json!(0), ApprovalKind::ApprovedDenied, true)["result"]["decision"],
            "approved"
        );
        assert_eq!(
            approval_reply(&json!(0), ApprovalKind::ApprovedDenied, false)["result"]["decision"],
            "denied"
        );
    }

    #[test]
    fn drain_takes_every_unanswered_request() {
        let mut pending = PendingRpcs::default();
        let cid = Uuid::new_v4();
        pending.insert(1, "turn/start", Some(cid), Instant::now() + RPC_TIMEOUT);
        pending.insert(2, "model/list", None, Instant::now() + RPC_TIMEOUT);
        let mut drained = pending.drain();
        drained.sort_by_key(|(id, _)| *id);
        assert_eq!(drained.len(), 2);
        assert_eq!(drained[0].1.command_id, Some(cid));
        assert!(pending.drain().is_empty());
    }

    // --- correlated JSON-RPC outcomes -----------------------------

    #[test]
    fn pending_rpcs_resolves_success_response() {
        let mut table = PendingRpcs::default();
        table.insert(100, "turn/start", None, Instant::now() + RPC_TIMEOUT);
        let resp = json!({"id": 100, "result": {"turn": {"id": "t1"}}});
        let (pending, outcome) = table.resolve(100, &resp).expect("pending entry");
        assert_eq!(pending.method, "turn/start");
        assert_eq!(outcome.unwrap().pointer("/turn/id").and_then(Value::as_str), Some("t1"));
        assert!(table.is_empty());
        assert!(table.resolve(100, &resp).is_none(), "entry is one-shot");
    }

    #[test]
    fn pending_rpcs_propagates_error_response() {
        let mut table = PendingRpcs::default();
        let cid = Uuid::new_v4();
        table.insert(2, "thread/start", Some(cid), Instant::now() + HANDSHAKE_TIMEOUT);
        let resp = json!({"id": 2, "error": {"code": -32600, "message": "bad thread", "data": {"hint": "x"}}});
        let (pending, outcome) = table.resolve(2, &resp).expect("pending entry");
        assert_eq!(pending.command_id, Some(cid));
        assert!(pending.is_handshake());
        let err = outcome.unwrap_err();
        assert!(err.contains("-32600"), "{err}");
        assert!(err.contains("bad thread"), "{err}");
        assert!(err.contains("hint"), "{err}");
    }

    #[test]
    fn pending_rpcs_expires_only_past_deadline() {
        let mut table = PendingRpcs::default();
        let now = Instant::now();
        table.insert(1, "turn/start", None, now + Duration::from_secs(5));
        table.insert(2, "turn/interrupt", Some(Uuid::new_v4()), now + Duration::from_mins(1));
        let expired = table.expire(now + Duration::from_secs(30));
        assert_eq!(expired.len(), 1);
        assert_eq!(expired[0].0, 1);
        assert_eq!(expired[0].1.method, "turn/start");
        assert!(!table.is_empty());
        assert_eq!(table.expire(now + Duration::from_mins(2)).len(), 1);
        assert!(table.is_empty());
    }

    #[test]
    fn pending_rpcs_drain_cancels_everything_on_process_exit() {
        let mut table = PendingRpcs::default();
        let now = Instant::now();
        table.insert(1, "initialize", None, now + RPC_TIMEOUT);
        table.insert(100, "turn/start", None, now + RPC_TIMEOUT);
        table.insert(101, "turn/interrupt", Some(Uuid::new_v4()), now + RPC_TIMEOUT);
        let drained = table.drain();
        assert_eq!(drained.len(), 3);
        assert!(table.is_empty());
        assert_eq!(drained.iter().filter(|(_, p)| p.command_id.is_some()).count(), 1);
    }

    #[test]
    fn response_outcome_shapes() {
        assert_eq!(
            response_outcome(&json!({"id": 1, "result": {"ok": 1}})).unwrap(),
            json!({"ok": 1})
        );
        assert_eq!(response_outcome(&json!({"id": 1})).unwrap(), Value::Null);
        let err = response_outcome(&json!({"id": 1, "error": {"message": "nope"}})).unwrap_err();
        assert!(err.contains("nope"), "{err}");
    }

    #[test]
    fn handshake_methods_are_flagged() {
        for m in ["initialize", "thread/start", "thread/resume", "thread/fork"] {
            let p = PendingRpc { method: m.to_owned(), command_id: None, deadline: Instant::now() };
            assert!(p.is_handshake(), "{m}");
        }
        let p = PendingRpc {
            method: "turn/start".to_owned(),
            command_id: None,
            deadline: Instant::now(),
        };
        assert!(!p.is_handshake());
    }
}
