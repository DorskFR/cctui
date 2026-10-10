//! `/cctuiverse/v1/*`: the routes a peer server calls, authenticated by the
//! link's signature alone. Unknown, closed, expired and badly signed links all
//! answer the same 404.

use axum::Json;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use super::handshake::{self, JoinResponse, not_found};
use super::{CloseReason, Link, LinkKind, LinkRole, LinkState, MAX_TEXT_BYTES, client, sig};
use crate::error::AppError;
use crate::routes::device_auth::PeerAddr;
use crate::routes::peer::Delivery;
use crate::state::AppState;

const IN_PER_MIN: usize = 30;
const DEFAULT_HISTORY_EVENTS: i64 = 200;
const MAX_HISTORY_EVENTS: i64 = 1_000;
const HISTORY_BUDGET_BYTES: usize = 64 * 1024;
const ROOM_SNAPSHOT: i64 = 50;

pub async fn fresh_nonce(pool: &sqlx::PgPool, link_id: Uuid, nonce: &str) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "INSERT INTO cctuiverse_nonces (link_id, nonce) VALUES ($1, $2) ON CONFLICT DO NOTHING",
    )
    .bind(link_id)
    .bind(nonce)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

async fn verified(
    state: &AppState,
    raw_id: &str,
    uri: &Uri,
    headers: &HeaderMap,
    body: &[u8],
    allow_expired: bool,
) -> Result<Link, AppError> {
    if !super::enabled(state) {
        return Err(not_found());
    }
    let id = Uuid::parse_str(raw_id).map_err(|_| not_found())?;
    let limiter = crate::routes::peer::limiter();
    if !limiter.admit(&format!("cv-in:{id}"), IN_PER_MIN, std::time::Instant::now()) {
        return Err(AppError::new(StatusCode::TOO_MANY_REQUESTS, "too many requests"));
    }
    let link = super::load(&state.pool, id).await?.ok_or_else(not_found)?;
    if link.state != LinkState::Active || (!allow_expired && link.expired(chrono::Utc::now())) {
        return Err(not_found());
    }
    let (Some(peer_key), Some(peer_id)) = (link.peer_public_key.as_deref(), link.peer_link_id)
    else {
        return Err(not_found());
    };
    let signed = sig::from_headers(headers).ok_or_else(not_found)?;
    let path = handshake::signed_path(&state.config.external_url, uri.path());
    let nonce =
        sig::verify(&signed, "POST", &path, body, chrono::Utc::now().timestamp(), peer_id, peer_key)
            .map_err(|_| not_found())?;
    if !fresh_nonce(&state.pool, id, &nonce).await? {
        return Err(not_found());
    }
    Ok(link)
}

/// `POST /cctuiverse/v1/join`.
pub async fn join(
    State(state): State<AppState>,
    PeerAddr(peer): PeerAddr,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<JoinResponse>, AppError> {
    let caller = crate::routes::device_auth::caller_key(&headers, peer, state.config.trusted_proxy_hops);
    handshake::accept(&state, &caller, uri.path(), &headers, &body).await.map(Json)
}

#[derive(Debug, Deserialize)]
struct MessageIn {
    message_id: Uuid,
    kind: String,
    text: String,
    #[serde(default)]
    room_name: Option<String>,
    #[serde(default)]
    sender_label: Option<String>,
}

fn bad_request(msg: impl Into<String>) -> AppError {
    AppError::new(StatusCode::BAD_REQUEST, msg)
}

fn accepted() -> (StatusCode, Json<Value>) {
    (StatusCode::ACCEPTED, Json(json!({ "status": "accepted" })))
}

#[must_use]
pub fn inbound_turn(link: &Link, kind: &str, body: &Value) -> Option<String> {
    let text = body["text"].as_str()?;
    match (link.kind, kind) {
        (LinkKind::Session, "direct") => Some(crate::envelope_guard::remote_envelope(
            link.id,
            link.peer_name(),
            &link.envelope_nonce,
            text,
        )),
        (LinkKind::Room, "room_post") => Some(crate::rooms::envelope(
            body["room_name"].as_str()?,
            body["sender_label"].as_str()?,
            text,
        )),
        _ => None,
    }
}

/// Maps a local delivery to the status the sender acts on: 409 is permanent,
/// 503 is retried.
fn delivery_status(d: Delivery) -> Result<(), AppError> {
    match d {
        Delivery::Delivered => Ok(()),
        Delivery::Archived | Delivery::Ended => {
            Err(AppError::new(StatusCode::CONFLICT, "peer session unavailable"))
        }
        Delivery::Offline => Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "peer session offline")),
    }
}

async fn set_status(pool: &sqlx::PgPool, row: i64, status: &str) {
    let _ = sqlx::query(
        "UPDATE cctuiverse_messages SET status = $2, \
             delivered_at = CASE WHEN $2 IN ('delivered', 'released') THEN now() END \
         WHERE id = $1",
    )
    .bind(row)
    .bind(status)
    .execute(pool)
    .await;
}

async fn forget(pool: &sqlx::PgPool, row: i64) {
    let _ = sqlx::query("DELETE FROM cctuiverse_messages WHERE id = $1").bind(row).execute(pool).await;
}

enum Inbound {
    Session,
    JoinerRoom,
    Host(Uuid),
}

/// `POST /cctuiverse/v1/links/{id}/messages`.
pub async fn messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let link = verified(&state, &id, &uri, &headers, &body, false).await?;
    let msg: MessageIn = serde_json::from_slice(&body).map_err(|_| bad_request("malformed message"))?;
    if msg.text.trim().is_empty() || msg.text.len() > MAX_TEXT_BYTES {
        return Err(bad_request(format!("text must be 1..={MAX_TEXT_BYTES} bytes")));
    }
    crate::envelope_guard::check(&msg.text).map_err(bad_request)?;
    let route = match (link.kind, link.role, msg.kind.as_str(), link.room_id, &link.session_id) {
        (LinkKind::Session, _, "direct", _, Some(_)) => Inbound::Session,
        (LinkKind::Room, LinkRole::Joiner, "room_post", _, Some(_)) => Inbound::JoinerRoom,
        (LinkKind::Room, LinkRole::Inviter, "room_post", Some(room), _) => Inbound::Host(room),
        _ => return Err(not_found()),
    };
    let stored = match route {
        Inbound::JoinerRoom => {
            let (Some(room), Some(sender)) = (
                msg.room_name.as_deref().and_then(super::clean_label),
                msg.sender_label.as_deref().and_then(super::clean_label),
            ) else {
                return Err(bad_request("room_name and sender_label are required"));
            };
            json!({ "room_name": room, "sender_label": sender, "text": msg.text })
        }
        Inbound::Session | Inbound::Host(_) => json!({ "text": msg.text }),
    };
    let hold = !matches!(route, Inbound::Host(_))
        && link.settings.inbound == super::InboundMode::Hold;
    let row: Option<i64> = sqlx::query_scalar(
        "INSERT INTO cctuiverse_messages (link_id, message_id, direction, kind, body, status) \
         VALUES ($1, $2, 'in', $3, $4, $5) \
         ON CONFLICT (link_id, direction, message_id) DO NOTHING RETURNING id",
    )
    .bind(link.id)
    .bind(msg.message_id)
    .bind(&msg.kind)
    .bind(&stored)
    .bind(if hold { "held" } else { "delivered" })
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else { return Ok(accepted()) };
    if hold {
        super::publish_changed(&state, &link);
        return Ok(accepted());
    }
    let delivery = match route {
        Inbound::Host(room) => {
            let sender = format!("{} (remote)", link.peer_name());
            if let Err(e) =
                crate::rooms::fanout_remote_post(&state, room, link.id, &sender, &msg.text).await
            {
                forget(&state.pool, row).await;
                return Err(e);
            }
            Delivery::Delivered
        }
        Inbound::Session | Inbound::JoinerRoom => {
            let (Some(sid), Some(turn)) =
                (link.session_id.as_deref(), inbound_turn(&link, &msg.kind, &stored))
            else {
                forget(&state.pool, row).await;
                return Err(not_found());
            };
            super::deliver_local(&state, sid, turn).await
        }
    };
    match delivery {
        Delivery::Delivered => {}
        Delivery::Offline => forget(&state.pool, row).await,
        Delivery::Archived | Delivery::Ended => set_status(&state.pool, row, "failed").await,
    }
    delivery_status(delivery)?;
    Ok(accepted())
}

/// `POST /cctuiverse/v1/links/{id}/close`.
pub async fn close(
    State(state): State<AppState>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let link = verified(&state, &id, &uri, &headers, &body, true).await?;
    super::close(&state, &link, CloseReason::Peer).await?;
    Ok(Json(json!({ "closed": true })))
}

#[derive(Debug, Default, Deserialize)]
struct HistoryIn {
    #[serde(default)]
    before: Option<i64>,
    #[serde(default)]
    limit: Option<i64>,
}

/// `POST /cctuiverse/v1/links/{id}/history`.
pub async fn history(
    State(state): State<AppState>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let link = verified(&state, &id, &uri, &headers, &body, false).await?;
    let (LinkKind::Session, Some(sid)) = (link.kind, link.session_id.as_deref()) else {
        return Err(not_found());
    };
    if !link.settings.share_transcript {
        return Err(AppError::new(StatusCode::FORBIDDEN, "transcript not shared"));
    }
    let req: HistoryIn = serde_json::from_slice(&body).unwrap_or_default();
    let adapter: Option<String> = sqlx::query_scalar("SELECT adapter_id FROM sessions WHERE id = $1")
        .bind(sid)
        .fetch_optional(&state.pool)
        .await?
        .flatten();
    let query = crate::routes::sessions::ConversationQuery {
        limit: Some(req.limit.unwrap_or(DEFAULT_HISTORY_EVENTS).clamp(1, MAX_HISTORY_EVENTS)),
        before: req.before,
        after: None,
        order: crate::routes::sessions::ConversationOrder::Desc,
    };
    let adapter = adapter.unwrap_or_else(|| "claude-code".to_owned());
    let mut rows =
        crate::routes::sessions::renderable_rows(&state.pool, sid, &adapter, &query).await?;
    rows.reverse();
    let events: Vec<(i64, Value)> = rows.into_iter().map(|(id, v, _, _)| (id, v)).collect();
    let st = super::session_state(&state.pool, sid).await.unwrap_or("archived");
    let header = crate::transcript_md::Header {
        session_id: link.label.clone(),
        name: Some(link.label.clone()),
        adapter: None,
        machine: None,
        state: st,
    };
    let rendered = crate::transcript_md::render(&header, &events, &[], HISTORY_BUDGET_BYTES);
    super::audit(
        &state.pool,
        sid,
        &format!("{} read this session's history — {} events", link.peer_name(), rendered.events),
    )
    .await;
    Ok(Json(json!({
        "name": link.label,
        "state": st,
        "events": rendered.events,
        "first_seq": rendered.first_seq,
        "last_seq": rendered.last_seq,
        "truncated": rendered.truncated,
        "markdown": rendered.text,
    })))
}

/// `POST /cctuiverse/v1/links/{id}/room`.
pub async fn room(
    State(state): State<AppState>,
    Path(id): Path<String>,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Json<Value>, AppError> {
    let link = verified(&state, &id, &uri, &headers, &body, false).await?;
    let (LinkKind::Room, LinkRole::Inviter, Some(room_id)) = (link.kind, link.role, link.room_id)
    else {
        return Err(not_found());
    };
    let name = super::room_name(&state.pool, room_id).await.ok_or_else(not_found)?;
    let mut members: Vec<String> = crate::rooms::members(&state.pool, room_id)
        .await?
        .into_iter()
        .filter(|m| m.state != "archived")
        .map(|m| m.name.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "session".to_owned()))
        .collect();
    for other in super::room_links(&state.pool, room_id).await? {
        if other.id != link.id {
            members.push(format!("{} (remote)", other.peer_name()));
        }
    }
    type Row = (i64, String, String, chrono::DateTime<chrono::Utc>);
    let mut rows: Vec<Row> = sqlx::query_as(
        "SELECT seq, sender_label, body, created_at FROM room_messages \
         WHERE room_id = $1 ORDER BY seq DESC LIMIT $2",
    )
    .bind(room_id)
    .bind(ROOM_SNAPSHOT)
    .fetch_all(&state.pool)
    .await?;
    rows.reverse();
    let messages: Vec<Value> = rows
        .into_iter()
        .map(|(seq, sender_label, body, created_at)| {
            json!({ "seq": seq, "sender_label": sender_label, "body": body, "created_at": created_at })
        })
        .collect();
    Ok(Json(json!({ "room_name": name, "members": members, "messages": messages })))
}

async fn ask_peer(state: &AppState, link: &Link, route: &str, body: &Value) -> Result<Value, AppError> {
    if !super::enabled(state) {
        return Err(AppError::new(StatusCode::NOT_FOUND, "cctuiverse is disabled on this server"));
    }
    if !link.usable() {
        return Err(AppError::new(StatusCode::CONFLICT, "the cctuiverse link is closed"));
    }
    let (Some(seed), Some(url), Some(peer_id)) =
        (link.seed(), link.peer_url.as_deref(), link.peer_link_id)
    else {
        return Err(AppError::new(StatusCode::CONFLICT, "the cctuiverse link is closed"));
    };
    let path = format!("/cctuiverse/v1/links/{peer_id}/{route}");
    match client::post_signed(state, &seed, link.id, url, &path, body).await {
        Ok((status, bytes)) if status.is_success() => serde_json::from_slice(&bytes)
            .map_err(|_| AppError::new(StatusCode::BAD_GATEWAY, "the peer answered garbage")),
        Ok((StatusCode::FORBIDDEN, _)) => {
            Err(AppError::new(StatusCode::FORBIDDEN, "the peer does not share its transcript"))
        }
        Ok((StatusCode::NOT_FOUND, _)) => {
            Err(AppError::new(StatusCode::CONFLICT, "the peer no longer recognises this link"))
        }
        Ok((status, _)) => Err(AppError::new(StatusCode::BAD_GATEWAY, format!("the peer answered {status}"))),
        Err(e) => {
            tracing::info!(link = %link.id, "cctuiverse peer unreachable: {e}");
            Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "the peer server is unreachable"))
        }
    }
}

/// Signed history read from the peer (session links). Err carries the peer's refusal.
pub async fn peer_history(
    state: &AppState,
    link: &Link,
    before: Option<i64>,
    limit: Option<i64>,
) -> Result<Value, AppError> {
    if link.kind != LinkKind::Session {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "this is a room link: use CctuiRoom peek"));
    }
    let mut out = ask_peer(state, link, "history", &json!({ "before": before, "limit": limit })).await?;
    out["session_id"] = json!(super::remote_ref(link.id));
    out["name"] = json!(link.peer_name());
    out["relation"] = json!("remote");
    Ok(out)
}

/// Signed room snapshot from the host (joiner side of a room link).
pub async fn peer_room(state: &AppState, link: &Link) -> Result<Value, AppError> {
    if link.kind != LinkKind::Room || link.role != LinkRole::Joiner {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "this link is not a remote room"));
    }
    ask_peer(state, link, "room", &json!({})).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_delivery_maps_to_the_status_the_sender_acts_on() {
        assert!(delivery_status(Delivery::Delivered).is_ok());
        for (d, code) in [
            (Delivery::Archived, StatusCode::CONFLICT),
            (Delivery::Ended, StatusCode::CONFLICT),
            (Delivery::Offline, StatusCode::SERVICE_UNAVAILABLE),
        ] {
            assert_eq!(delivery_status(d).unwrap_err().status(), code);
        }
    }
}
