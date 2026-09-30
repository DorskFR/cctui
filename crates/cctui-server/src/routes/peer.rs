//! The `/api/v1/daemon/sessions/{id}/peers|message-peer|peer-conversation`
//! family — the server side of the `CctuiPeers`, `CctuiSend` and `CctuiHistory`
//! MCP tools.
//!
//! Authenticated by the caller's machine key like the rest of the daemon family,
//! authorised by [`crate::peer_policy`], and routed by [`crate::bus::dispatch`],
//! which resolves the TARGET's machine — so a peer on another machine or another
//! replica is reached by the same call as one next door.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use dashmap::DashMap;
use serde_json::{Value, json};

use cctui_proto::adapter::AdapterCommand;
use cctui_proto::api::PeerMessageRequest;

use crate::error::AppError;
use crate::peer_policy::{self, Refusal, Relation, SessionNode};
use crate::state::AppState;
use crate::transcript_md;

/// Per-sender ceilings. A peer message is a turn in somebody else's session:
/// cheap to send, expensive to receive.
pub(crate) const SEND_PER_MIN: usize = 10;
const HISTORY_PER_MIN: usize = 30;
const WINDOW: Duration = Duration::from_secs(60);

/// A peer message becomes a prompt, so it is capped like one. A room broadcast
/// is the same message to several targets, so it is capped and counted the same:
/// one broadcast spends one send from the caller's window.
pub(crate) const MAX_MESSAGE_BYTES: usize = 32 * 1024;

/// Default and maximum page of a history read.
const DEFAULT_HISTORY_EVENTS: i64 = 200;
const MAX_HISTORY_EVENTS: i64 = 1_000;
const HISTORY_BUDGET_BYTES: usize = 64 * 1024;

/// A closing envelope tag inside the body would end the wrapper early and the
/// remainder would render as the sender's own prose in the target's transcript.
const ENVELOPE_CLOSE: &str = "</cross-session-message>";

/// Sliding-window counter, one window per key.
#[derive(Default)]
pub struct Limiter(DashMap<String, VecDeque<Instant>>);

impl Limiter {
    /// Record a call and report whether it is within `max` per [`WINDOW`].
    pub fn admit(&self, key: &str, max: usize, now: Instant) -> bool {
        let mut hits = self.0.entry(key.to_owned()).or_default();
        while hits.front().is_some_and(|t| now.duration_since(*t) >= WINDOW) {
            hits.pop_front();
        }
        if hits.len() >= max {
            return false;
        }
        hits.push_back(now);
        true
    }
}

pub(crate) fn limiter() -> &'static Limiter {
    static LIMITER: std::sync::LazyLock<Limiter> = std::sync::LazyLock::new(Limiter::default);
    &LIMITER
}

fn refuse(r: Refusal) -> AppError {
    AppError::new(r.status(), r.reason())
}

/// Authenticate the machine key, then authorize `caller → target`, recording
/// every refusal with both session ids.
async fn authorized(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    caller_id: &str,
    target_id: &str,
) -> Result<(Relation, SessionNode, SessionNode), AppError> {
    let owner = crate::routes::spawn_child::machine_user(state, headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    match peer_policy::authorize(&state.pool, caller_id, target_id, owner).await {
        Ok((relation, facts)) => Ok((relation, facts.caller, facts.target)),
        Err(refusal) => {
            tracing::warn!(
                caller = %caller_id,
                target = %target_id,
                reason = refusal.reason(),
                "peer address refused",
            );
            Err(refuse(refusal))
        }
    }
}

/// A markdown-attribute-safe rendering of a label.
fn attr(raw: &str) -> String {
    raw.replace(['"', '<', '>'], "")
}

/// Wrap `body` in the envelope the webui's peer-message detector recognises
/// (`PEER_TAG_RE`, `webui/.../conversation/format.ts`) and the server's own
/// [`crate::normalize::client_category`] files under the `peer` role.
#[must_use]
pub fn envelope(sender: &SessionNode, body: &str) -> String {
    format!(
        "<cross-session-message from=\"{}\" from-name=\"{}\">\n{}\n{ENVELOPE_CLOSE}",
        attr(&sender.id),
        attr(&sender.label()),
        body.trim(),
    )
}

/// Record a marker turn on `session_id` so the human sees what an agent did on
/// their behalf. Best-effort: losing the audit row must not fail the call it
/// describes, but it is logged loudly when it does.
pub(crate) async fn audit(pool: &sqlx::PgPool, session_id: &str, text: &str) {
    let payload = json!({ "role": "system_marker", "text": text });
    if let Err(err) = sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2)",
    )
    .bind(session_id)
    .bind(&payload)
    .execute(pool)
    .await
    {
        tracing::error!(%session_id, %err, "peer audit event insert failed");
    }
}

/// What one delivery attempt did. `Offline` covers every reason the bus could not
/// place the turn: no daemon for the machine, no adapter, a closed channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Delivered,
    Archived,
    Ended,
    Offline,
}

impl Delivery {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Delivered => "delivered",
            Self::Archived => "archived",
            Self::Ended => "ended",
            Self::Offline => "offline",
        }
    }
}

/// Place `text` as a turn in `target_id`'s session — the ONE delivery path for
/// every agent-to-agent message, direct or broadcast.
///
/// `bus::dispatch` resolves the target's own machine and routes across replicas,
/// so this inherits whatever the bus transport becomes (CCT-568's NATS core would
/// swap underneath with no change here). A session mid-turn is not special-cased:
/// it receives the turn exactly as it would a message the human sent while it was
/// working.
pub async fn deliver(
    state: &AppState,
    target_id: &str,
    target_state: &str,
    text: String,
) -> Delivery {
    match target_state {
        "archived" => return Delivery::Archived,
        "ended" => return Delivery::Ended,
        _ => {}
    }
    match crate::bus::dispatch(
        state,
        target_id,
        AdapterCommand::SendMessage { local_id: target_id.to_owned(), text },
    )
    .await
    {
        Ok(()) => Delivery::Delivered,
        Err(err) => {
            tracing::info!(target = %target_id, %err, "peer turn not delivered");
            Delivery::Offline
        }
    }
}

/// `GET /api/v1/daemon/sessions/{id}/peers`.
pub async fn list_peers(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let owner = crate::routes::spawn_child::machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let me = peer_policy::load_node(&state.pool, &session_id)
        .await?
        .filter(|n| n.user_id == Some(owner))
        .ok_or_else(|| refuse(Refusal::Unknown))?;
    let rows: Vec<peer_policy::RosterRow> =
        sqlx::query_as(peer_policy::ROSTER_SQL).bind(&me.id).fetch_all(&state.pool).await?;
    let peers: Vec<peer_policy::Peer> = rows
        .into_iter()
        .map(|(id, name, adapter, machine, status, relation)| peer_policy::Peer {
            session_id: id,
            name,
            adapter,
            machine,
            state: peer_policy::state_of(status.as_deref()),
            relation,
        })
        .collect();
    Ok(Json(json!({ "session_id": me.id, "peers": peers })))
}

/// `POST /api/v1/daemon/sessions/{id}/message-peer`.
pub async fn message_peer(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<PeerMessageRequest>,
) -> Result<Json<Value>, AppError> {
    let target_id = req.session_id.trim();
    if target_id.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session_id is required"));
    }
    let body = req.message.trim();
    if body.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "message is required"));
    }
    if body.len() > MAX_MESSAGE_BYTES {
        return Err(AppError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "message is {} bytes; the peer-message cap is {MAX_MESSAGE_BYTES}. Send a pointer \
                 (a path, a session id) rather than a payload.",
                body.len()
            ),
        ));
    }
    if body.contains(ENVELOPE_CLOSE) {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("message must not contain {ENVELOPE_CLOSE}: it would truncate the envelope"),
        ));
    }
    let (relation, caller, target) = authorized(&state, &headers, &session_id, target_id).await?;
    if relation == Relation::Own {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "a session cannot message itself"));
    }
    if !target.is_live() {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            format!(
                "session {target_id} is {} and cannot receive a message. Its transcript is still \
                 readable with CctuiHistory.",
                target.state()
            ),
        ));
    }
    if !limiter().admit(&format!("send:{session_id}"), SEND_PER_MIN, Instant::now()) {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("peer-message rate limit reached ({SEND_PER_MIN} per minute per session)"),
        ));
    }

    let text = envelope(&caller, body);
    match deliver(&state, &target.id, target.state(), text).await {
        Delivery::Delivered => {}
        outcome => {
            return Err(AppError::new(
                StatusCode::SERVICE_UNAVAILABLE,
                format!("could not deliver to {target_id}: {}", outcome.as_str()),
            ));
        }
    }
    audit(
        &state.pool,
        &session_id,
        &format!("sent a peer message to {} [{}]", target.label(), relation.as_str()),
    )
    .await;
    tracing::info!(
        caller = %session_id,
        target = %target.id,
        relation = relation.as_str(),
        "peer message delivered",
    );
    Ok(Json(json!({ "delivered_to": target.id, "relation": relation.as_str() })))
}

/// The history route's query string. Flat, with no `#[serde(flatten)]`:
/// `serde_urlencoded`, which axum's `Query` uses, cannot deserialize a flattened
/// struct at all.
#[derive(Debug, Default, serde::Deserialize)]
pub struct PeerConversationParams {
    pub session_id: String,
    pub after: Option<i64>,
    pub before: Option<i64>,
    pub limit: Option<i64>,
    /// Comma-separated role list; empty keeps every role.
    pub roles: Option<String>,
    #[serde(default)]
    pub format: HistoryFormat,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryFormat {
    #[default]
    Markdown,
    Json,
}

/// Split a `roles=` value into the role names [`crate::normalize::client_category`]
/// emits. Unknown names are kept: they simply match nothing, which is a clearer
/// outcome than a 400 for a model guessing at the vocabulary.
fn parse_roles(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// `GET /api/v1/daemon/sessions/{id}/peer-conversation`.
///
/// Reads `stream_events` from Postgres only, so it works for an archived session
/// and for one whose machine no longer exists.
pub async fn peer_conversation(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Query(params): Query<PeerConversationParams>,
) -> Result<Json<Value>, AppError> {
    let target_id = params.session_id.trim();
    if target_id.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "session_id is required"));
    }
    let (relation, _caller, target) = authorized(&state, &headers, &session_id, target_id).await?;
    if !limiter().admit(&format!("history:{session_id}"), HISTORY_PER_MIN, Instant::now()) {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("peer-history rate limit reached ({HISTORY_PER_MIN} per minute per session)"),
        ));
    }
    let conversation = crate::routes::sessions::ConversationQuery {
        limit: Some(params.limit.unwrap_or(DEFAULT_HISTORY_EVENTS).clamp(1, MAX_HISTORY_EVENTS)),
        before: params.before,
        after: params.after,
        order: if params.after.is_some() {
            crate::routes::sessions::ConversationOrder::Asc
        } else {
            crate::routes::sessions::ConversationOrder::Desc
        },
    };
    let adapter = target.adapter_id.clone().unwrap_or_else(|| "claude-code".to_owned());
    let mut rows =
        crate::routes::sessions::renderable_rows(&state.pool, &target.id, &adapter, &conversation)
            .await?;
    if conversation.order == crate::routes::sessions::ConversationOrder::Desc {
        rows.reverse();
    }
    let events: Vec<(i64, Value)> = rows.into_iter().map(|(id, v, _, _)| (id, v)).collect();
    let header = transcript_md::Header {
        session_id: target.id.clone(),
        name: target.name.clone(),
        adapter: target.adapter_id.clone(),
        machine: target.machine_name.clone(),
        state: target.state(),
    };
    let roles = parse_roles(params.roles.as_deref());
    let rendered = transcript_md::render(&header, &events, &roles, HISTORY_BUDGET_BYTES);

    audit(
        &state.pool,
        &session_id,
        &format!(
            "consulted history of {} — {} events [{}]",
            target.label(),
            rendered.events,
            relation.as_str()
        ),
    )
    .await;

    let mut out = json!({
        "session_id": target.id,
        "name": target.name,
        "adapter": target.adapter_id,
        "machine": target.machine_name,
        "state": target.state(),
        "relation": relation.as_str(),
        "events": rendered.events,
        "first_seq": rendered.first_seq,
        "last_seq": rendered.last_seq,
        "truncated": rendered.truncated,
    });
    match params.format {
        HistoryFormat::Markdown => out["markdown"] = json!(rendered.text),
        HistoryFormat::Json => {
            let kept: Vec<Value> = events
                .into_iter()
                .filter_map(|(seq, payload)| {
                    let (role, _) = crate::normalize::client_category(&payload);
                    (!role.is_empty() && (roles.is_empty() || roles.iter().any(|r| r == role)))
                        .then(|| json!({ "seq": seq, "role": role, "payload": payload }))
                })
                .collect();
            out["events"] = json!(kept.len());
            out["items"] = json!(kept);
        }
    }
    Ok(Json(out))
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn node(id: &str, name: Option<&str>) -> SessionNode {
        SessionNode {
            id: id.to_owned(),
            user_id: Some(Uuid::nil()),
            parent_id: None,
            machine_uuid: None,
            machine_name: Some("box-a".into()),
            adapter_id: Some("claude-code".into()),
            name: name.map(str::to_owned),
            status: Some("active".into()),
            room_id: None,
        }
    }

    /// The envelope must be exactly what the webui's `PEER_TAG_RE` matches and
    /// what `normalize::client_category` files as `peer`; otherwise the message
    /// renders as a plain human turn in the target's transcript.
    #[test]
    fn the_envelope_is_the_shape_the_renderers_detect() {
        let text = envelope(&node("sess-1", Some("lane a")), "check the tests");
        assert!(text.starts_with("<cross-session-message from=\"sess-1\" from-name=\""), "{text}");
        assert!(text.contains("from-name=\"lane a (claude-code on box-a)\""), "{text}");
        assert!(text.ends_with("</cross-session-message>"), "{text}");
        assert!(text.contains("check the tests"));

        let as_stored = json!({ "type": "text", "content": format!("▷ User: {text}") });
        assert_eq!(crate::normalize::client_category(&as_stored).0, "peer");
    }

    #[test]
    fn a_quote_in_a_session_name_cannot_break_out_of_the_envelope_attributes() {
        let text = envelope(&node("s\"1", Some("a\" onload=<x>")), "hi");
        let head = text.lines().next().unwrap();
        assert_eq!(head.matches('"').count(), 4, "{head}");
        assert!(!head.contains("onload=<"), "{head}");
        assert_eq!(
            crate::normalize::client_category(&json!({
                "type": "text", "content": format!("▷ User: {text}"),
            }))
            .0,
            "peer"
        );
    }

    #[test]
    fn the_window_admits_up_to_the_cap_then_refuses_until_it_slides() {
        let limiter = Limiter::default();
        let t0 = Instant::now();
        for i in 0..SEND_PER_MIN {
            assert!(limiter.admit("send:a", SEND_PER_MIN, t0), "call {i} must be admitted");
        }
        assert!(!limiter.admit("send:a", SEND_PER_MIN, t0), "the cap must hold");
        assert!(limiter.admit("send:b", SEND_PER_MIN, t0), "the window is per sender");
        assert!(
            limiter.admit("send:a", SEND_PER_MIN, t0 + WINDOW + Duration::from_secs(1)),
            "the window must slide"
        );
    }

    #[test]
    fn send_and_history_windows_are_counted_separately() {
        let limiter = Limiter::default();
        let t0 = Instant::now();
        for _ in 0..SEND_PER_MIN {
            assert!(limiter.admit("send:a", SEND_PER_MIN, t0));
        }
        assert!(!limiter.admit("send:a", SEND_PER_MIN, t0));
        assert!(limiter.admit("history:a", HISTORY_PER_MIN, t0));
    }

    #[test]
    fn roles_parse_from_a_comma_list() {
        assert_eq!(parse_roles(Some("user, Assistant ,tool")), vec!["user", "assistant", "tool"]);
        assert!(parse_roles(None).is_empty());
        assert!(parse_roles(Some("  , ")).is_empty());
    }

    #[test]
    fn the_history_query_parses_its_format_and_defaults_to_markdown() {
        let params: PeerConversationParams = serde_json::from_value(json!({
            "session_id": "s1", "limit": 5, "before": 90, "format": "json",
            "roles": "user,assistant",
        }))
        .unwrap();
        assert_eq!(params.session_id, "s1");
        assert_eq!(params.limit, Some(5));
        assert_eq!(params.before, Some(90));
        assert_eq!(params.format, HistoryFormat::Json);
        assert_eq!(parse_roles(params.roles.as_deref()), vec!["user", "assistant"]);

        let bare: PeerConversationParams =
            serde_json::from_value(json!({ "session_id": "s1" })).unwrap();
        assert_eq!(bare.format, HistoryFormat::Markdown);
        assert!(bare.limit.is_none() && bare.after.is_none() && bare.roles.is_none());
    }

    /// DB-gated end-to-end policy + history: a child reads the transcript of an
    /// ARCHIVED parent whose machine row is gone, a sibling on another machine
    /// is addressable, and an unrelated session of the same owner is 403.
    #[tokio::test]
    async fn history_reads_an_archived_parent_and_the_policy_holds_across_machines() {
        let Some(url) = crate::routes::gateway::test_db_url("peer_history_and_policy") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let machine_a = Uuid::new_v4();
        let machine_b = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'peer-test', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        for m in [machine_a, machine_b] {
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(m)
            .bind(uid)
            .bind(format!("box-{m}"))
            .bind(format!("kh-{m}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        }
        let parent = Uuid::new_v4().to_string();
        let child_a = Uuid::new_v4().to_string();
        let child_b = Uuid::new_v4().to_string();
        let loner = Uuid::new_v4().to_string();
        for (id, par, m, status) in [
            (&parent, None::<&str>, machine_a, "archived"),
            (&child_a, Some(parent.as_str()), machine_a, "active"),
            (&child_b, Some(parent.as_str()), machine_b, "active"),
            (&loner, None, machine_b, "active"),
        ] {
            sqlx::query(
                "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
                 machine_uuid, adapter_id, session_name, status) \
                 VALUES ($1, $2, $3, '/w', $4, $5, 'claude-code', $6, $7)",
            )
            .bind(id)
            .bind(par)
            .bind(m.to_string())
            .bind(uid)
            .bind(m)
            .bind(format!("name-{id}"))
            .bind(status)
            .execute(&pool)
            .await
            .expect("seed session");
        }

        assert_eq!(
            peer_policy::authorize(&pool, &child_a, &parent, uid).await.unwrap().0,
            Relation::Parent,
        );
        assert_eq!(
            peer_policy::authorize(&pool, &child_b, &child_a, uid).await.unwrap().0,
            Relation::Sibling,
            "a sibling on another machine is addressable: the policy never looks at machines",
        );
        assert_eq!(
            peer_policy::authorize(&pool, &child_a, &loner, uid).await.unwrap_err(),
            Refusal::Unrelated,
        );
        assert_eq!(
            peer_policy::authorize(&pool, &child_a, &loner, Uuid::new_v4()).await.unwrap_err(),
            Refusal::Unknown,
            "another owner's machine key must not resolve this caller at all",
        );

        // A room is the only explicit grant: putting the unrelated pair in one
        // makes it addressable, and taking either out takes the right away.
        let room_id: Uuid = sqlx::query_scalar(
            "INSERT INTO rooms (user_id, name) VALUES ($1, 'shared') RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .expect("seed room");
        sqlx::query("UPDATE sessions SET room_id = $1 WHERE id = ANY($2)")
            .bind(room_id)
            .bind(vec![loner.clone(), child_a.clone()])
            .execute(&pool)
            .await
            .expect("seed room members");
        assert_eq!(
            peer_policy::authorize(&pool, &child_a, &loner, uid).await.unwrap().0,
            Relation::Room,
            "a room relates both directions",
        );
        assert_eq!(
            peer_policy::authorize(&pool, &loner, &child_a, uid).await.unwrap().0,
            Relation::Room,
        );
        sqlx::query("UPDATE sessions SET room_id = NULL WHERE id = $1")
            .bind(&loner)
            .execute(&pool)
            .await
            .expect("leave room");
        assert_eq!(
            peer_policy::authorize(&pool, &child_a, &loner, uid).await.unwrap_err(),
            Refusal::Unrelated,
        );

        let roster: Vec<peer_policy::RosterRow> = sqlx::query_as(peer_policy::ROSTER_SQL)
            .bind(&child_a)
            .fetch_all(&pool)
            .await
            .expect("roster");
        let by_id: std::collections::HashMap<&str, &str> =
            roster.iter().map(|r| (r.0.as_str(), r.5.as_str())).collect();
        assert_eq!(by_id.get(parent.as_str()), Some(&"parent"));
        assert_eq!(by_id.get(child_b.as_str()), Some(&"sibling"));
        assert!(!by_id.contains_key(loner.as_str()), "a session out of the room is off the roster");
        assert!(!by_id.contains_key(child_a.as_str()), "the caller is not its own peer");

        // The archived parent's transcript is still in stream_events, and the
        // machine going away does not touch it.
        for (role, text) in [
            ("user", "plan the migration"),
            ("assistant", "here is the plan"),
            ("assistant", "and the follow-up"),
        ] {
            sqlx::query(
                "INSERT INTO stream_events (session_id, event_type, payload) \
                 VALUES ($1, 'message', $2)",
            )
            .bind(&parent)
            .bind(json!({ "role": role, "text": text }))
            .execute(&pool)
            .await
            .expect("seed event");
        }
        let q = crate::routes::sessions::ConversationQuery {
            limit: Some(DEFAULT_HISTORY_EVENTS),
            ..Default::default()
        };
        let mut rows = crate::routes::sessions::renderable_rows(&pool, &parent, "claude-code", &q)
            .await
            .expect("rows");
        rows.reverse();
        let events: Vec<(i64, Value)> = rows.into_iter().map(|(id, v, _, _)| (id, v)).collect();
        let node = peer_policy::load_node(&pool, &parent).await.unwrap().unwrap();
        assert_eq!(node.state(), "archived");
        assert!(!node.is_live(), "an archived target must refuse a send");
        let header = transcript_md::Header {
            session_id: node.id.clone(),
            name: node.name.clone(),
            adapter: node.adapter_id.clone(),
            machine: node.machine_name.clone(),
            state: node.state(),
        };
        let all = transcript_md::render(&header, &events, &[], HISTORY_BUDGET_BYTES);
        assert_eq!(all.events, 3, "{}", all.text);
        assert!(all.text.contains("plan the migration"), "{}", all.text);
        assert!(all.text.contains("state: archived"), "{}", all.text);

        let only_user =
            transcript_md::render(&header, &events, &["user".into()], HISTORY_BUDGET_BYTES);
        assert_eq!(only_user.events, 1);

        // Pagination: one event at a time, walking back with `before`.
        let page =
            crate::routes::sessions::ConversationQuery { limit: Some(1), ..Default::default() };
        let newest = crate::routes::sessions::renderable_rows(&pool, &parent, "claude-code", &page)
            .await
            .expect("newest page");
        assert_eq!(newest.len(), 1);
        let newest_seq = newest[0].0;
        let older = crate::routes::sessions::renderable_rows(
            &pool,
            &parent,
            "claude-code",
            &crate::routes::sessions::ConversationQuery {
                limit: Some(1),
                before: Some(newest_seq),
                ..Default::default()
            },
        )
        .await
        .expect("older page");
        assert_eq!(older.len(), 1);
        assert!(older[0].0 < newest_seq, "before must page backwards");

        audit(&pool, &child_a, "consulted history of the parent").await;
        let marker: Option<String> = sqlx::query_scalar(
            "SELECT payload->>'text' FROM stream_events \
             WHERE session_id = $1 AND payload->>'role' = 'system_marker' \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(&child_a)
        .fetch_optional(&pool)
        .await
        .expect("audit row");
        assert_eq!(marker.as_deref(), Some("consulted history of the parent"));

        sqlx::query("DELETE FROM rooms WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
