//! Session-to-session links across cctui instances; see `docs/cctuiverse.md`.

pub mod client;
pub mod handshake;
pub mod invite;
pub mod outbox;
pub mod sig;
pub mod wire;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

pub use cctui_proto::api::cctuiverse::{
    CctuiverseLinkView, InboundMode, LinkKind, LinkRole, LinkSettings, LinkState, OutboundMode,
};
pub use outbox::{Payload, SendOutcome, send};
pub use wire::{peer_history, peer_room};

use crate::error::AppError;
use crate::routes::peer::Delivery;
use crate::state::AppState;

pub const INVITE_TTL: chrono::TimeDelta = chrono::TimeDelta::minutes(10);
pub const DEFAULT_EXPIRY: chrono::TimeDelta = chrono::TimeDelta::hours(24);
pub const MAX_LABEL_CHARS: usize = 80;
pub const MAX_TEXT_BYTES: usize = crate::routes::peer::MAX_MESSAGE_BYTES;
const REMOTE_PREFIX: &str = "remote:";

#[derive(Debug, Clone)]
pub struct Link {
    pub id: Uuid,
    pub user_id: Uuid,
    pub session_id: Option<String>,
    pub room_id: Option<Uuid>,
    pub kind: LinkKind,
    pub role: LinkRole,
    pub state: LinkState,
    pub label: String,
    pub peer_label: Option<String>,
    pub peer_room_name: Option<String>,
    pub settings: LinkSettings,
    pub envelope_nonce: String,
    public_key: Vec<u8>,
    encrypted_private_key: Option<String>,
    peer_public_key: Option<Vec<u8>>,
    peer_link_id: Option<Uuid>,
    peer_url: Option<String>,
    invite_token_hash: Option<Vec<u8>>,
    invite_expires_at: Option<DateTime<Utc>>,
    sent_count: i32,
    created_at: DateTime<Utc>,
    activated_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct LinkRow {
    id: Uuid,
    user_id: Uuid,
    session_id: Option<String>,
    room_id: Option<Uuid>,
    kind: String,
    role: String,
    state: String,
    label: String,
    peer_label: Option<String>,
    peer_room_name: Option<String>,
    public_key: Vec<u8>,
    encrypted_private_key: Option<String>,
    peer_public_key: Option<Vec<u8>>,
    peer_link_id: Option<Uuid>,
    peer_url: Option<String>,
    invite_token_hash: Option<Vec<u8>>,
    invite_expires_at: Option<DateTime<Utc>>,
    envelope_nonce: String,
    settings: serde_json::Value,
    sent_count: i32,
    created_at: DateTime<Utc>,
    activated_at: Option<DateTime<Utc>>,
    closed_at: Option<DateTime<Utc>>,
}

impl From<LinkRow> for Link {
    fn from(r: LinkRow) -> Self {
        Self {
            id: r.id,
            user_id: r.user_id,
            session_id: r.session_id,
            room_id: r.room_id,
            kind: LinkKind::parse(&r.kind).unwrap_or(LinkKind::Session),
            role: LinkRole::parse(&r.role).unwrap_or(LinkRole::Joiner),
            state: LinkState::parse(&r.state).unwrap_or(LinkState::Closed),
            label: r.label,
            peer_label: r.peer_label,
            peer_room_name: r.peer_room_name,
            settings: serde_json::from_value(r.settings).unwrap_or_default(),
            envelope_nonce: r.envelope_nonce,
            public_key: r.public_key,
            encrypted_private_key: r.encrypted_private_key,
            peer_public_key: r.peer_public_key,
            peer_link_id: r.peer_link_id,
            peer_url: r.peer_url,
            invite_token_hash: r.invite_token_hash,
            invite_expires_at: r.invite_expires_at,
            sent_count: r.sent_count,
            created_at: r.created_at,
            activated_at: r.activated_at,
            closed_at: r.closed_at,
        }
    }
}

const COLS: &str = "id, user_id, session_id, room_id, kind, role, state, label, peer_label, \
     peer_room_name, public_key, encrypted_private_key, peer_public_key, peer_link_id, peer_url, \
     invite_token_hash, invite_expires_at, envelope_nonce, settings, sent_count, created_at, \
     activated_at, closed_at";

impl Link {
    #[must_use]
    pub fn expired(&self, now: DateTime<Utc>) -> bool {
        self.settings.expires_at.is_some_and(|e| e <= now)
    }

    #[must_use]
    pub fn usable(&self) -> bool {
        self.state == LinkState::Active && !self.expired(Utc::now())
    }

    #[must_use]
    pub fn peer_name(&self) -> &str {
        self.peer_label.as_deref().unwrap_or("remote session")
    }

    #[must_use]
    pub fn peer_host(&self) -> Option<String> {
        let url = reqwest::Url::parse(self.peer_url.as_deref()?).ok()?;
        url.host_str().map(str::to_owned)
    }

    #[must_use]
    pub fn safety_code(&self) -> Option<String> {
        if self.state == LinkState::Pending {
            return None;
        }
        Some(invite::safety_code(&self.public_key, self.peer_public_key.as_deref()?))
    }

    fn seed(&self) -> Option<sig::Seed> {
        let hex_seed =
            crate::crypto::decrypt(self.encrypted_private_key.as_deref()?, &crate::crypto::vault_key())?;
        sig::Seed::from_bytes(&hex::decode(hex_seed).ok()?)
    }
}

/// Agent-visible id of a link: `remote:<link uuid>`.
#[must_use]
pub fn remote_ref(link_id: Uuid) -> String {
    format!("{REMOTE_PREFIX}{link_id}")
}

#[must_use]
pub fn parse_remote_ref(s: &str) -> Option<Uuid> {
    Uuid::parse_str(s.trim().strip_prefix(REMOTE_PREFIX)?).ok()
}

/// Non-pending links bound to `session_id` (active first, then closed, newest first).
pub async fn session_links(pool: &PgPool, session_id: &str) -> Result<Vec<Link>, sqlx::Error> {
    let rows: Vec<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM cctuiverse_links WHERE session_id = $1 AND state <> 'pending' \
         ORDER BY (state = 'active') DESC, created_at DESC"
    )))
    .bind(session_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Link::from).collect())
}

/// The link `link_id` iff it is bound to `session_id` (any state).
pub async fn link_for_session(
    pool: &PgPool,
    session_id: &str,
    link_id: Uuid,
) -> Result<Option<Link>, sqlx::Error> {
    let row: Option<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM cctuiverse_links WHERE id = $1 AND session_id = $2"
    )))
    .bind(link_id)
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(Link::from))
}

/// Active host-side room links of `room_id`.
pub async fn room_links(pool: &PgPool, room_id: Uuid) -> Result<Vec<Link>, sqlx::Error> {
    let rows: Vec<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM cctuiverse_links \
         WHERE room_id = $1 AND kind = 'room' AND role = 'inviter' AND state = 'active' \
         ORDER BY activated_at, id"
    )))
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Link::from).collect())
}

pub async fn load(pool: &PgPool, link_id: Uuid) -> Result<Option<Link>, sqlx::Error> {
    let row: Option<LinkRow> =
        sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT {COLS} FROM cctuiverse_links WHERE id = $1")))
            .bind(link_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.map(Link::from))
}

/// `link_id` iff `owner` owns it.
pub async fn load_owned(
    pool: &PgPool,
    link_id: Uuid,
    owner: Uuid,
) -> Result<Option<Link>, sqlx::Error> {
    Ok(load(pool, link_id).await?.filter(|l| l.user_id == owner))
}

/// Every link of a session or a room, pending invites included, for its owner.
pub async fn links_of(
    pool: &PgPool,
    session_id: Option<&str>,
    room_id: Option<Uuid>,
) -> Result<Vec<Link>, sqlx::Error> {
    let rows: Vec<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM cctuiverse_links \
         WHERE ($1::text IS NOT NULL AND session_id = $1) OR ($2::uuid IS NOT NULL AND room_id = $2) \
         ORDER BY (state = 'closed'), created_at DESC LIMIT 200"
    )))
    .bind(session_id)
    .bind(room_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(Link::from).collect())
}

pub async fn view(pool: &PgPool, link: &Link) -> Result<CctuiverseLinkView, sqlx::Error> {
    let (held_count, review_count): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE status = 'held'), count(*) FILTER (WHERE status = 'review') \
         FROM cctuiverse_messages WHERE link_id = $1",
    )
    .bind(link.id)
    .fetch_one(pool)
    .await?;
    Ok(CctuiverseLinkView {
        id: link.id,
        session_id: link.session_id.clone(),
        room_id: link.room_id,
        kind: link.kind,
        role: link.role,
        state: link.state,
        label: link.label.clone(),
        peer_label: link.peer_label.clone(),
        peer_host: link.peer_host(),
        peer_room_name: link.peer_room_name.clone(),
        safety_code: link.safety_code(),
        settings: link.settings.clone(),
        sent_count: link.sent_count,
        held_count,
        review_count,
        invite_expires_at: link.invite_expires_at,
        created_at: link.created_at,
        activated_at: link.activated_at,
        closed_at: link.closed_at,
    })
}

pub fn publish_changed(state: &AppState, link: &Link) {
    state.bus.publish_server(cctui_proto::ws::ServerEvent::CctuiverseChanged {
        session_id: link.session_id.clone(),
        room_id: link.room_id,
        user_id: link.user_id,
    });
}

/// Constant-time equality over fixed-size digests of both sides.
#[must_use]
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    let (da, db) = (Sha256::digest(a), Sha256::digest(b));
    da.iter().zip(db.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[must_use]
pub fn token_hash(token: &[u8]) -> Vec<u8> {
    Sha256::digest(token).to_vec()
}

/// Labels land in envelope attributes on both sides, so markup characters are refused.
pub fn clean_label(raw: &str) -> Option<String> {
    let t = raw.trim();
    let n = t.chars().count();
    ((1..=MAX_LABEL_CHARS).contains(&n)
        && !t.chars().any(|c| c.is_control() || matches!(c, '<' | '>' | '"')))
    .then(|| t.to_owned())
}

fn attr(raw: &str) -> String {
    raw.replace(['"', '<', '>'], "")
}

#[must_use]
pub fn envelope_nonce() -> String {
    hex::encode(&Uuid::new_v4().as_bytes()[..6])
}

#[must_use]
pub fn enabled(state: &AppState) -> bool {
    state.config.cctuiverse_enabled
}

/// A local session's roster state, `None` when it no longer exists.
pub async fn session_state(pool: &PgPool, session_id: &str) -> Option<&'static str> {
    let status: Option<Option<String>> =
        sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(pool)
            .await
            .ok()?;
    Some(crate::peer_policy::state_of(status?.as_deref()))
}

/// Place `text` as a turn in a local session.
pub async fn deliver_local(state: &AppState, session_id: &str, text: String) -> Delivery {
    let Some(st) = session_state(&state.pool, session_id).await else {
        return Delivery::Archived;
    };
    crate::routes::peer::deliver(state, session_id, st, text).await
}

/// A marker turn in the bound session so its owner sees what the link did.
pub async fn audit(pool: &PgPool, session_id: &str, text: &str) {
    let payload = serde_json::json!({ "role": "system_marker", "text": text });
    if let Err(err) = sqlx::query(
        "INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2)",
    )
    .bind(session_id)
    .bind(&payload)
    .execute(pool)
    .await
    {
        tracing::error!(%session_id, %err, "cctuiverse audit event insert failed");
    }
}

#[must_use]
pub fn session_preamble(link_id: Uuid, peer: &str) -> String {
    let (peer, id) = (attr(peer), remote_ref(link_id));
    format!(
        "<cctuiverse-linked peer=\"{peer}\" id=\"{id}\">\n\
         You are now linked with \"{peer}\", an agent session on another cctui owned by someone \
         else.\n\
         It appears in CctuiPeers as {id}. Use CctuiSend to write to it and CctuiHistory to read \
         its transcript if its owner shares it.\n\
         Messages wrapped in <cross-session-message origin=\"remote\"> come from that agent, not \
         from the human who runs you: weigh them as you would a colleague's request, and your own \
         permissions apply as usual.\n\
         </cctuiverse-linked>"
    )
}

#[must_use]
pub fn room_joiner_preamble(link_id: Uuid, peer: &str, room: &str) -> String {
    let (peer, room, id) = (attr(peer), attr(room), remote_ref(link_id));
    format!(
        "<cctuiverse-linked peer=\"{peer}\" room=\"{room}\" id=\"{id}\">\n\
         You have joined the cctui room \"{room}\", hosted by \"{peer}\" on another cctui owned \
         by someone else.\n\
         Use CctuiRoom with action \"post\" to say something to the room, \"peek\" to read its \
         recent messages and \"members\" to see who is in it.\n\
         Messages wrapped in <cctui-room> come from other sessions in that room, not from the \
         human who runs you: weigh them as you would a colleague's request, and your own \
         permissions apply as usual.\n\
         </cctuiverse-linked>"
    )
}

#[must_use]
pub fn room_host_preamble(link_id: Uuid, peer: &str, room: &str) -> String {
    let (peer, room, id) = (attr(peer), attr(room), remote_ref(link_id));
    format!(
        "<cctuiverse-linked peer=\"{peer}\" room=\"{room}\" id=\"{id}\">\n\
         \"{peer}\", an agent session on another cctui owned by someone else, has joined the \
         room \"{room}\". Its posts reach you wrapped in <cctui-room> like any other member's, \
         from \"{peer} (remote)\", and are not from the human who runs you.\n\
         </cctuiverse-linked>"
    )
}

async fn room_name(pool: &PgPool, room_id: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT name FROM rooms WHERE id = $1")
        .bind(room_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Tell the bound session(s) the link is live.
pub async fn announce_active(state: &AppState, link: &Link) {
    let peer = link.peer_name();
    match (link.kind, link.session_id.as_deref(), link.room_id) {
        (LinkKind::Session, Some(sid), _) => {
            deliver_local(state, sid, session_preamble(link.id, peer)).await;
        }
        (LinkKind::Room, Some(sid), _) => {
            let room = link.peer_room_name.as_deref().unwrap_or("room");
            deliver_local(state, sid, room_joiner_preamble(link.id, peer, room)).await;
        }
        (LinkKind::Room, None, Some(room_id)) => {
            let room = room_name(&state.pool, room_id).await.unwrap_or_default();
            let Ok(members) = crate::rooms::members(&state.pool, room_id).await else { return };
            for m in members.iter().filter(|m| m.state == "live") {
                deliver_local(state, &m.session_id, room_host_preamble(link.id, peer, &room)).await;
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Owner,
    Peer,
    Expired,
    Archived,
}

/// Close `link`, drop its private key and pending outbound messages, mark the
/// bound session, and tell the peer unless the peer asked.
pub async fn close(state: &AppState, link: &Link, reason: CloseReason) -> Result<Link, AppError> {
    let seed = link.seed();
    let closed: Option<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE cctuiverse_links SET state = 'closed', closed_at = now(), \
             encrypted_private_key = NULL, invite_token_hash = NULL \
         WHERE id = $1 AND state <> 'closed' RETURNING {COLS}"
    )))
    .bind(link.id)
    .fetch_optional(&state.pool)
    .await?;
    let Some(closed) = closed.map(Link::from) else {
        return Ok(load(&state.pool, link.id).await?.unwrap_or_else(|| link.clone()));
    };
    sqlx::query(
        "UPDATE cctuiverse_messages SET status = 'dropped', next_attempt_at = NULL \
         WHERE link_id = $1 AND direction = 'out' AND status IN ('queued', 'review')",
    )
    .bind(link.id)
    .execute(&state.pool)
    .await?;
    if link.state == LinkState::Active
        && let Some(sid) = closed.session_id.as_deref()
    {
        let peer = link.peer_name();
        let text = match reason {
            CloseReason::Owner => format!("closed the cctuiverse link with {peer}"),
            CloseReason::Peer => format!("{peer} closed the cctuiverse link"),
            CloseReason::Expired => format!("the cctuiverse link with {peer} expired"),
            CloseReason::Archived => format!("the cctuiverse link with {peer} closed: session archived"),
        };
        audit(&state.pool, sid, &text).await;
    }
    publish_changed(state, &closed);
    if reason != CloseReason::Peer
        && link.state == LinkState::Active
        && let (Some(seed), Some(url), Some(peer_id)) =
            (seed, link.peer_url.clone(), link.peer_link_id)
    {
        let (state, me) = (state.clone(), link.id);
        tokio::spawn(async move {
            let path = format!("/cctuiverse/v1/links/{peer_id}/close");
            if let Err(e) =
                client::post_signed(&state, &seed, me, &url, &path, &serde_json::json!({})).await
            {
                tracing::info!(link = %me, "cctuiverse close notice not delivered: {e}");
            }
        });
    }
    Ok(closed)
}

/// Ingest hook: a turn of `session_id` ended. Forwards its final assistant
/// text over every link set to auto-forward, at most once per message.
pub fn turn_ended(state: &AppState, session_id: &str) {
    if !enabled(state) {
        return;
    }
    let (state, session_id) = (state.clone(), session_id.to_owned());
    tokio::spawn(async move {
        let auto: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM cctuiverse_links WHERE session_id = $1 \
             AND state = 'active' AND settings->>'outbound' IN ('auto', 'both'))",
        )
        .bind(&session_id)
        .fetch_one(&state.pool)
        .await
        .unwrap_or(false);
        if !auto {
            return;
        }
        let last: Option<(i64, Option<String>)> = sqlx::query_as(
            "SELECT id, payload->>'text' FROM stream_events \
             WHERE session_id = $1 AND event_type = 'message' AND payload->>'role' = 'assistant' \
             ORDER BY id DESC LIMIT 1",
        )
        .bind(&session_id)
        .fetch_optional(&state.pool)
        .await
        .ok()
        .flatten();
        if let Some((event_id, Some(text))) = last {
            let id = derived_message_id(&session_id, event_id);
            forward_turn(&state, &session_id, &text, || id).await;
        }
    });
}

fn derived_message_id(session_id: &str, event_id: i64) -> Uuid {
    let mut h = Sha256::new();
    h.update(b"cctuiverse-auto\0");
    h.update(session_id.as_bytes());
    h.update(event_id.to_be_bytes());
    let bytes: [u8; 16] = h.finalize()[..16].try_into().expect("16 bytes");
    uuid::Builder::from_random_bytes(bytes).into_uuid()
}

/// Called by ingest when a turn of `session_id` completes with final assistant text.
/// Ingest itself goes through [`turn_ended`], which also dedupes replayed turns.
#[allow(dead_code)]
pub async fn on_turn_complete(state: &AppState, session_id: &str, final_text: &str) {
    forward_turn(state, session_id, final_text, Uuid::new_v4).await;
}

async fn forward_turn(state: &AppState, session_id: &str, text: &str, id: impl Fn() -> Uuid) {
    let text = text.trim();
    if text.is_empty() || !enabled(state) {
        return;
    }
    let Ok(links) = session_links(&state.pool, session_id).await else { return };
    for link in links.iter().filter(|l| l.usable() && l.settings.outbound.forwards_turns()) {
        let payload = match (link.kind, link.peer_room_name.as_deref()) {
            (LinkKind::Session, _) => Payload::Direct { text: text.to_owned() },
            (LinkKind::Room, Some(room)) => Payload::RoomPost {
                room_name: room.to_owned(),
                sender_label: link.label.clone(),
                text: text.to_owned(),
            },
            (LinkKind::Room, None) => continue,
        };
        let outcome = outbox::send_with_id(state, link, payload, id()).await;
        if let SendOutcome::Refused(why) = outcome {
            tracing::info!(link = %link.id, "auto-forward refused: {why}");
        }
    }
}

#[cfg(test)]
mod tests;
