//! GH-AGENT-2: the MCP review tool exposed to a **review agent session**.
//!
//! A review session is a normal cctui session; it talks to this server as an
//! HTTP MCP server registered in the worker's `~/.mcp.json` (the worker's
//! credential helper maps `MCP_<NAME>_URL` / `MCP_<NAME>_TOKEN` →
//! `{type:"http", url, headers:{Authorization:"Bearer <token>"}}`). That is the
//! *existing* way cctui hands a tool surface to a session — we reuse it rather
//! than invent a transport. The bearer is the session's own session token (the
//! same `cctui_s_*` opaque token minted for the LLM gateway), so the agent is
//! authenticated as exactly the session it runs in.
//!
//! Two tools, both writing **drafts only** — nothing reaches GitHub (publish
//! stays the human's GH-VIEW-5 action):
//!   * `review_comment` — add one inline draft comment, anchored on the
//!     GH-VIEW-2 `(path, side, line[, start_line])` coordinates, into the
//!     session's agent draft for a PR.
//!   * `review_summary` — set/append the draft's summary and set its verdict
//!     (`comment` | `approve` | `request_changes`).
//!
//! Auth → author: the session token resolves (via `public.session_tokens`) to a
//! `session_id`, which becomes the draft's `author_session_id`
//! (`author_kind = 'agent'`). A token that resolves to no live session is
//! rejected — the agent can only write under its own session. No token is ever
//! logged.
//!
//! Transport: a single stateless JSON-RPC 2.0 POST handler implementing the MCP
//! 2025-06-18 streamable-HTTP contract (`initialize`, `notifications/*`,
//! `tools/list`, `tools/call`) — no session id, no SSE, one JSON response per
//! POST.

use axum::Json;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use cctui_proto::github::{CreateDraftComment, DiffSide, ReviewVerdict};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::GithubState;
use crate::drafts::{self, DraftError};

/// The MCP protocol revision we speak.
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Resolve a bearer session token to the session id it was minted for.
///
/// Mirrors the gateway's `session_tokens` lookup (CCT-232): only a non-revoked
/// token resolves. The token itself is never returned or logged — only the
/// `session_id` it maps to. `None` ⇒ unknown/revoked ⇒ the caller maps it to an
/// auth error.
async fn resolve_session(state: &GithubState, token: &str) -> Option<String> {
    let hash = cctui_proto::util::sha256_hex(token.as_bytes());
    sqlx::query_scalar::<_, String>(
        "SELECT session_id FROM public.session_tokens \
         WHERE token_hash = $1 AND revoked_at IS NULL",
    )
    .bind(hash)
    .fetch_optional(&state.pool)
    .await
    .ok()
    .flatten()
}

/// Extract the bearer token from the `Authorization` header.
fn bearer(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::to_owned)
}

/// The two tool descriptors, as MCP `tools/list` entries. Pulled out so the
/// shape is unit-testable without standing up HTTP.
pub fn tool_descriptors() -> Value {
    json!([
        {
            "name": "review_comment",
            "description": "Add one inline DRAFT review comment to the current pull \
                request, anchored to a diff line. The comment is staged in cctui's \
                local review draft for your session — it is NOT posted to GitHub \
                (a human publishes the batched review later). Call once per line \
                you want to comment on.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connector_id": { "type": "string", "description": "The GitHub connector UUID for this PR (from the session's PR context)." },
                    "repo": { "type": "string", "description": "The repository slug 'owner/name'." },
                    "pull_number": { "type": "integer", "description": "The pull request number." },
                    "path": { "type": "string", "description": "Head-side file path the comment anchors on." },
                    "side": { "type": "string", "enum": ["old", "new"], "description": "Which diff side the line is on. 'new' (head) is the usual choice." },
                    "line": { "type": "integer", "minimum": 1, "description": "1-based line number on `side` (the END line for a multi-line range)." },
                    "start_line": { "type": "integer", "minimum": 1, "description": "Optional inclusive start line for a multi-line range; must be <= line and on the same side." },
                    "body": { "type": "string", "description": "The comment text (markdown)." }
                },
                "required": ["connector_id", "repo", "pull_number", "path", "side", "line", "body"]
            }
        },
        {
            "name": "review_summary",
            "description": "Set or append your review's overall summary and verdict \
                for the current pull request. Verdict is one of comment | approve | \
                request_changes. This is staged in your local review draft — it is \
                NOT submitted to GitHub.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "connector_id": { "type": "string", "description": "The GitHub connector UUID for this PR." },
                    "repo": { "type": "string", "description": "The repository slug 'owner/name'." },
                    "pull_number": { "type": "integer", "description": "The pull request number." },
                    "summary": { "type": "string", "description": "The review summary text (markdown)." },
                    "verdict": { "type": "string", "enum": ["comment", "approve", "request_changes"], "description": "The pending verdict. Defaults to 'comment'." },
                    "append": { "type": "boolean", "description": "If true, append to any existing summary instead of replacing it. Defaults to false." }
                },
                "required": ["connector_id", "repo", "pull_number", "summary"]
            }
        }
    ])
}

/// Parse a [`DiffSide`] from the tool argument string.
fn parse_side(s: &str) -> Option<DiffSide> {
    match s {
        "old" => Some(DiffSide::Old),
        "new" => Some(DiffSide::New),
        _ => None,
    }
}

/// Parse a [`ReviewVerdict`] from the tool argument string.
fn parse_verdict(s: &str) -> Option<ReviewVerdict> {
    match s {
        "comment" => Some(ReviewVerdict::Comment),
        "approve" => Some(ReviewVerdict::Approve),
        "request_changes" => Some(ReviewVerdict::RequestChanges),
        _ => None,
    }
}

/// A `u32` line number from a JSON integer, rejecting <1 / non-integers.
fn parse_line(v: Option<&Value>) -> Option<u32> {
    let n = v?.as_u64()?;
    if n == 0 || n > u64::from(u32::MAX) {
        return None;
    }
    u32::try_from(n).ok()
}

/// Outcome of validating + applying one tool call (pure of HTTP). `Ok(text)` is
/// the human-readable success message; `Err(text)` is a tool error
/// (`isError: true`) the agent sees and can recover from.
pub enum ToolOutcome {
    Ok(String),
    Err(String),
}

/// Common PR-locator fields shared by both tools, parsed + validated.
struct PullRef {
    connector_id: Uuid,
    repo: String,
    number: i64,
}

fn parse_pull_ref(args: &Value) -> Result<PullRef, String> {
    let connector_id =
        args.get("connector_id").and_then(Value::as_str).ok_or("connector_id is required")?;
    let connector_id = Uuid::parse_str(connector_id)
        .map_err(|_| "connector_id is not a valid UUID".to_string())?;
    let repo = args
        .get("repo")
        .and_then(Value::as_str)
        .filter(|s| s.contains('/') && !s.is_empty())
        .ok_or("repo is required and must be 'owner/name'")?
        .to_owned();
    let number = args
        .get("pull_number")
        .and_then(Value::as_i64)
        .filter(|n| *n > 0)
        .ok_or("pull_number is required and must be a positive integer")?;
    Ok(PullRef { connector_id, repo, number })
}

/// Apply `review_comment`: open-or-reuse the session's agent draft, then add the
/// anchored comment. `session_id` is the authenticated author.
async fn do_review_comment(state: &GithubState, session_id: &str, args: &Value) -> ToolOutcome {
    let pr = match parse_pull_ref(args) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::Err(e),
    };
    let Some(path) = args.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return ToolOutcome::Err("path is required".into());
    };
    let Some(side) = args.get("side").and_then(Value::as_str).and_then(parse_side) else {
        return ToolOutcome::Err("side is required and must be 'old' or 'new'".into());
    };
    let Some(line) = parse_line(args.get("line")) else {
        return ToolOutcome::Err("line is required and must be a positive integer".into());
    };
    let start_line = match args.get("start_line") {
        None | Some(Value::Null) => None,
        Some(v) => match parse_line(Some(v)) {
            Some(n) => Some(n),
            None => return ToolOutcome::Err("start_line must be a positive integer".into()),
        },
    };
    // Anchor validation: a range's start must not run past its end (the same
    // invariant GH-VIEW-2 enforces; both endpoints are on `side`).
    if start_line.is_some_and(|start| start > line) {
        return ToolOutcome::Err("start_line must be <= line".into());
    }
    let Some(body) = args.get("body").and_then(Value::as_str).filter(|s| !s.is_empty()) else {
        return ToolOutcome::Err("body is required and must be non-empty".into());
    };

    let draft = match drafts::open_agent_draft(
        &state.pool,
        pr.connector_id,
        &pr.repo,
        pr.number,
        session_id,
        ReviewVerdict::Comment,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => return draft_err(e),
    };

    let comment = CreateDraftComment {
        path: path.to_owned(),
        side,
        line,
        start_line,
        body: body.to_owned(),
        in_reply_to: None,
    };
    match drafts::add_comment(&state.pool, pr.connector_id, &pr.repo, pr.number, draft.id, &comment)
        .await
    {
        Ok(d) => ToolOutcome::Ok(format!(
            "Draft comment staged on {}:{} ({} total in this review draft). Not yet posted to GitHub.",
            path,
            line,
            d.comments.len()
        )),
        Err(e) => draft_err(e),
    }
}

/// Apply `review_summary`: set/append the session's agent draft summary +
/// verdict.
async fn do_review_summary(state: &GithubState, session_id: &str, args: &Value) -> ToolOutcome {
    let pr = match parse_pull_ref(args) {
        Ok(p) => p,
        Err(e) => return ToolOutcome::Err(e),
    };
    let Some(summary) = args.get("summary").and_then(Value::as_str).filter(|s| !s.is_empty())
    else {
        return ToolOutcome::Err("summary is required and must be non-empty".into());
    };
    let verdict = match args.get("verdict") {
        None | Some(Value::Null) => ReviewVerdict::Comment,
        Some(Value::String(s)) => match parse_verdict(s) {
            Some(v) => v,
            None => {
                return ToolOutcome::Err(
                    "verdict must be 'comment', 'approve', or 'request_changes'".into(),
                );
            }
        },
        Some(_) => return ToolOutcome::Err("verdict must be a string".into()),
    };
    let append = args.get("append").and_then(Value::as_bool).unwrap_or(false);

    let draft = match drafts::open_agent_draft(
        &state.pool,
        pr.connector_id,
        &pr.repo,
        pr.number,
        session_id,
        verdict,
    )
    .await
    {
        Ok(d) => d,
        Err(e) => return draft_err(e),
    };

    match drafts::set_summary(
        &state.pool,
        pr.connector_id,
        &pr.repo,
        pr.number,
        draft.id,
        &drafts::SummaryUpdate { summary, verdict, append },
    )
    .await
    {
        Ok(_) => ToolOutcome::Ok(format!(
            "Review summary {} (verdict: {}). Staged in your draft, not yet submitted.",
            if append { "appended" } else { "set" },
            verdict_label(verdict)
        )),
        Err(e) => draft_err(e),
    }
}

const fn verdict_label(v: ReviewVerdict) -> &'static str {
    match v {
        ReviewVerdict::Comment => "comment",
        ReviewVerdict::Approve => "approve",
        ReviewVerdict::RequestChanges => "request_changes",
    }
}

/// Map a draft-store error to a tool error message (no internals leaked).
fn draft_err(e: DraftError) -> ToolOutcome {
    match e {
        DraftError::NotFound => {
            ToolOutcome::Err("the target review draft was not found or is already published".into())
        }
        DraftError::Db => {
            ToolOutcome::Err("a storage error occurred while staging the draft".into())
        }
    }
}

/// Build a successful JSON-RPC `tools/call` result from a [`ToolOutcome`].
fn tool_result(outcome: ToolOutcome) -> Value {
    let (text, is_error) = match outcome {
        ToolOutcome::Ok(t) => (t, false),
        ToolOutcome::Err(t) => (t, true),
    };
    json!({
        "content": [ { "type": "text", "text": text } ],
        "isError": is_error
    })
}

/// JSON-RPC error response helper.
fn rpc_error(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

/// JSON-RPC success response helper.
fn rpc_result(id: &Value, result: &Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

/// `POST /api/v1/github/mcp` — the stateless MCP endpoint.
///
/// Authenticates the bearer session token, then dispatches the JSON-RPC method.
/// `initialize` / `tools/list` need only a valid token; `tools/call` resolves it
/// to the author session id. Notifications return `202 Accepted` with no body.
pub async fn handler(
    State(state): State<GithubState>,
    headers: HeaderMap,
    Json(req): Json<Value>,
) -> Response {
    // Auth first: a missing/unknown token is rejected before we look at the
    // body, so an unauthenticated caller learns nothing about the tools.
    let Some(token) = bearer(&headers) else {
        return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
    };
    let Some(session_id) = resolve_session(&state, &token).await else {
        return (StatusCode::UNAUTHORIZED, "invalid or revoked session token").into_response();
    };

    let method = req.get("method").and_then(Value::as_str).unwrap_or_default();
    let id = req.get("id").cloned().unwrap_or(Value::Null);

    // A notification (no `id`) is fire-and-forget — ack with 202, no body.
    if req.get("id").is_none() {
        return StatusCode::ACCEPTED.into_response();
    }

    let body = match method {
        "initialize" => rpc_result(
            &id,
            &json!({
                "protocolVersion": PROTOCOL_VERSION,
                "serverInfo": { "name": "cctui-github-review", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "tools": { "listChanged": false } }
            }),
        ),
        "tools/list" => rpc_result(&id, &json!({ "tools": tool_descriptors() })),
        "tools/call" => {
            let params = req.get("params").cloned().unwrap_or(Value::Null);
            let name = params.get("name").and_then(Value::as_str).unwrap_or_default();
            let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let outcome = match name {
                "review_comment" => do_review_comment(&state, &session_id, &args).await,
                "review_summary" => do_review_summary(&state, &session_id, &args).await,
                other => {
                    return Json(rpc_error(&id, -32601, &format!("unknown tool: {other}")))
                        .into_response();
                }
            };
            rpc_result(&id, &tool_result(outcome))
        }
        other => rpc_error(&id, -32601, &format!("method not found: {other}")),
    };

    Json(body).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_descriptors_expose_both_tools_with_required_args() {
        let tools = tool_descriptors();
        let arr = tools.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        let names: Vec<&str> =
            arr.iter().filter_map(|t| t.get("name").and_then(Value::as_str)).collect();
        assert!(names.contains(&"review_comment"));
        assert!(names.contains(&"review_summary"));
        for t in arr {
            // Every tool has an object input schema with a non-empty `required`.
            let schema = t.get("inputSchema").unwrap();
            assert_eq!(schema.get("type").unwrap(), "object");
            assert!(!schema.get("required").unwrap().as_array().unwrap().is_empty());
        }
    }

    #[test]
    fn parse_side_and_verdict_round_trip_and_reject() {
        assert_eq!(parse_side("old"), Some(DiffSide::Old));
        assert_eq!(parse_side("new"), Some(DiffSide::New));
        assert_eq!(parse_side("LEFT"), None);
        assert_eq!(parse_verdict("approve"), Some(ReviewVerdict::Approve));
        assert_eq!(parse_verdict("request_changes"), Some(ReviewVerdict::RequestChanges));
        assert_eq!(parse_verdict("comment"), Some(ReviewVerdict::Comment));
        assert_eq!(parse_verdict("lgtm"), None);
    }

    #[test]
    fn parse_line_rejects_zero_and_non_integers() {
        assert_eq!(parse_line(Some(&json!(1))), Some(1));
        assert_eq!(parse_line(Some(&json!(42))), Some(42));
        assert_eq!(parse_line(Some(&json!(0))), None);
        assert_eq!(parse_line(Some(&json!(-3))), None);
        assert_eq!(parse_line(Some(&json!("5"))), None);
        assert_eq!(parse_line(None), None);
    }

    #[test]
    fn parse_pull_ref_validates_shape() {
        let ok =
            json!({ "connector_id": Uuid::nil().to_string(), "repo": "o/n", "pull_number": 7 });
        let pr = parse_pull_ref(&ok).unwrap();
        assert_eq!(pr.repo, "o/n");
        assert_eq!(pr.number, 7);

        // Bad UUID.
        assert!(
            parse_pull_ref(&json!({ "connector_id": "nope", "repo": "o/n", "pull_number": 7 }))
                .is_err()
        );
        // repo without a slash.
        assert!(parse_pull_ref(&json!({ "connector_id": Uuid::nil().to_string(), "repo": "owner", "pull_number": 7 })).is_err());
        // non-positive pull number.
        assert!(
            parse_pull_ref(
                &json!({ "connector_id": Uuid::nil().to_string(), "repo": "o/n", "pull_number": 0 })
            )
            .is_err()
        );
    }

    #[test]
    fn tool_result_marks_errors() {
        let ok = tool_result(ToolOutcome::Ok("done".into()));
        assert_eq!(ok["isError"], false);
        assert_eq!(ok["content"][0]["text"], "done");
        let err = tool_result(ToolOutcome::Err("boom".into()));
        assert_eq!(err["isError"], true);
        assert_eq!(err["content"][0]["text"], "boom");
    }
}
