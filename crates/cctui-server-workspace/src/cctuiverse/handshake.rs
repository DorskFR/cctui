use axum::http::{HeaderMap, StatusCode};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::sig::{self, Seed};
use super::{
    COLS, DEFAULT_EXPIRY, DEFAULT_MAX_MESSAGES, INVITE_TTL, Link, LinkKind, LinkRole, LinkRow,
    LinkState, client, invite, publish_changed,
};
use crate::error::AppError;
use crate::state::AppState;

pub const JOIN_ROUTE: &str = "/cctuiverse/v1/join";
const MAX_PENDING_INVITES: i64 = 20;
const JOIN_PER_LINK_PER_MIN: usize = 5;
pub const REFUSED: &str = "invite invalid, expired or already used";

#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    Session(&'a str),
    Room(Uuid),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinRequest {
    pub link_id: Uuid,
    pub token: String,
    pub joiner: Joiner,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Joiner {
    pub link_id: Uuid,
    pub url: String,
    pub public_key: String,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct JoinResponse {
    pub link_id: Uuid,
    pub public_key: String,
    pub label: String,
    pub kind: LinkKind,
    pub room_name: Option<String>,
}

pub(super) fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not found")
}

fn sealed(seed: &Seed) -> String {
    crate::crypto::encrypt(&hex::encode(seed.as_bytes()), &crate::crypto::vault_key())
}

fn b64_key(raw: &str) -> Option<[u8; 32]> {
    URL_SAFE_NO_PAD.decode(raw).ok()?.try_into().ok()
}

pub async fn create_invite(
    state: &AppState,
    user_id: Uuid,
    target: Target<'_>,
    label: &str,
) -> Result<(Link, String), AppError> {
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM cctuiverse_links \
         WHERE user_id = $1 AND state = 'pending' AND role = 'inviter'",
    )
    .bind(user_id)
    .fetch_one(&state.pool)
    .await?;
    if pending >= MAX_PENDING_INVITES {
        return Err(AppError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!("at most {MAX_PENDING_INVITES} open invites at a time"),
        ));
    }
    let seed = Seed::generate();
    let public_key = seed.public_key();
    let token = sig::random_bytes::<32>();
    let (session_id, room_id, kind) = match target {
        Target::Session(id) => (Some(id), None, LinkKind::Session),
        Target::Room(id) => (None, Some(id), LinkKind::Room),
    };
    let row: LinkRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO cctuiverse_links \
             (id, user_id, session_id, room_id, kind, role, state, label, public_key, \
              encrypted_private_key, invite_token_hash, invite_expires_at, envelope_nonce, \
              settings) \
         VALUES ($1, $2, $3, $4, $5, 'inviter', 'pending', $6, $7, $8, $9, now() + $10, $11, \
                 jsonb_build_object('max_messages', $12::int)) \
         RETURNING {COLS}"
    )))
    .bind(Uuid::new_v4())
    .bind(user_id)
    .bind(session_id)
    .bind(room_id)
    .bind(kind.as_str())
    .bind(label)
    .bind(public_key.as_slice())
    .bind(sealed(&seed))
    .bind(super::token_hash(&token))
    .bind(INVITE_TTL)
    .bind(super::envelope_nonce())
    .bind(DEFAULT_MAX_MESSAGES)
    .fetch_one(&state.pool)
    .await?;
    let link = Link::from(row);
    let url = invite::format(&state.config.external_url, link.id, &token, &public_key);
    publish_changed(state, &link);
    Ok((link, url))
}

/// Joiner side: create our link for `session_id`, run the handshake against
/// the inviter, and activate on success. Any refusal leaves no row behind,
/// and once the inviter may have committed, a refusal also closes its side.
pub async fn join(
    state: &AppState,
    user_id: Uuid,
    raw_invite: &str,
    session_id: &str,
    label: &str,
) -> Result<Link, AppError> {
    let inv = invite::parse(raw_invite)
        .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "not a cctuiverse invite"))?;
    client::check_url(state, &inv.base_url).await.map_err(|e| {
        AppError::new(StatusCode::BAD_REQUEST, format!("the inviting server is not allowed: {e}"))
    })?;
    let seed = Seed::generate();
    let public_key = seed.public_key();
    let expires = chrono::Utc::now() + DEFAULT_EXPIRY;
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO cctuiverse_links \
             (id, user_id, session_id, kind, role, state, label, public_key, \
              encrypted_private_key, peer_link_id, peer_url, envelope_nonce, settings) \
         VALUES ($1, $2, $3, 'session', 'joiner', 'pending', $4, $5, $6, $7, $8, $9, \
                 jsonb_build_object('expires_at', $10::timestamptz, 'max_messages', $11::int))",
    )
    .bind(id)
    .bind(user_id)
    .bind(session_id)
    .bind(label)
    .bind(public_key.as_slice())
    .bind(sealed(&seed))
    .bind(inv.link_id)
    .bind(&inv.base_url)
    .bind(super::envelope_nonce())
    .bind(expires)
    .bind(DEFAULT_MAX_MESSAGES)
    .execute(&state.pool)
    .await?;

    let request = JoinRequest {
        link_id: inv.link_id,
        token: URL_SAFE_NO_PAD.encode(inv.token),
        joiner: Joiner {
            link_id: id,
            url: state.config.external_url.trim_end_matches('/').to_owned(),
            public_key: URL_SAFE_NO_PAD.encode(public_key),
            label: label.to_owned(),
        },
    };
    let body = serde_json::to_value(&request)?;
    let outcome = client::post_signed(state, &seed, id, &inv.base_url, JOIN_ROUTE, &body).await;
    let answer = match outcome {
        Err(e) => {
            tracing::info!("cctuiverse join could not reach {}: {e}", inv.base_url);
            withdraw(state, &seed, id).await;
            return Err(AppError::new(
                StatusCode::BAD_GATEWAY,
                "could not reach the inviting server",
            ));
        }
        Ok((status, bytes)) if status == StatusCode::OK => {
            let parsed = serde_json::from_slice::<JoinResponse>(&bytes).ok();
            if let Some(a) = parsed.and_then(|r| accepted(&inv, &r)) {
                a
            } else {
                withdraw(state, &seed, id).await;
                return Err(AppError::new(StatusCode::NOT_FOUND, REFUSED));
            }
        }
        Ok((StatusCode::NOT_FOUND | StatusCode::TOO_MANY_REQUESTS, _)) => {
            discard(state, id).await;
            return Err(AppError::new(StatusCode::NOT_FOUND, REFUSED));
        }
        Ok(_) => {
            withdraw(state, &seed, id).await;
            return Err(AppError::new(StatusCode::NOT_FOUND, REFUSED));
        }
    };
    let (peer_key, peer_label, kind, room_name) = answer;
    let row: Option<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE cctuiverse_links SET state = 'active', kind = $2, peer_public_key = $3, \
             peer_label = $4, peer_room_name = $5, activated_at = now(), preamble_pending = true \
         WHERE id = $1 AND state = 'pending' RETURNING {COLS}"
    )))
    .bind(id)
    .bind(kind.as_str())
    .bind(peer_key.as_slice())
    .bind(&peer_label)
    .bind(&room_name)
    .fetch_optional(&state.pool)
    .await?;
    let Some(row) = row else {
        withdraw(state, &seed, id).await;
        return Err(AppError::new(StatusCode::CONFLICT, "the join was cancelled"));
    };
    let link = Link::from(row);
    activated(state, &link);
    Ok(link)
}

/// Give up on a join the inviter may already have committed: close this side,
/// keeping (or restoring) its key until the queued close notice settles.
async fn withdraw(state: &AppState, seed: &Seed, id: Uuid) {
    let row: Result<Option<LinkRow>, _> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE cctuiverse_links SET state = 'closed', closed_at = COALESCE(closed_at, now()), \
             encrypted_private_key = $2, invite_token_hash = NULL \
         WHERE id = $1 AND state IN ('pending', 'closed') RETURNING {COLS}"
    )))
    .bind(id)
    .bind(sealed(seed))
    .fetch_optional(&state.pool)
    .await;
    match row {
        Ok(Some(row)) => super::outbox::enqueue_close(state, &Link::from(row)).await,
        Ok(None) => {}
        Err(e) => tracing::warn!(link = %id, "cctuiverse join withdrawal not recorded: {e}"),
    }
}

/// The inviter's answer, iff its key matches the fingerprint the invite carried.
/// Names are cleaned rather than refused: the inviter has already committed.
fn accepted(
    inv: &invite::Invite,
    r: &JoinResponse,
) -> Option<([u8; 32], String, LinkKind, Option<String>)> {
    let key = b64_key(&r.public_key)?;
    if r.link_id != inv.link_id || !super::ct_eq(&invite::fingerprint(&key), &inv.fingerprint) {
        return None;
    }
    let label = super::sanitize_label(&r.label).unwrap_or_else(|| "remote session".to_owned());
    let room = match r.kind {
        LinkKind::Session => None,
        LinkKind::Room => Some(
            r.room_name.as_deref().and_then(super::sanitize_label).unwrap_or_else(|| "room".into()),
        ),
    };
    Some((key, label, r.kind, room))
}

async fn discard(state: &AppState, id: Uuid) {
    let _ = sqlx::query("DELETE FROM cctuiverse_links WHERE id = $1 AND state = 'pending'")
        .bind(id)
        .execute(&state.pool)
        .await;
}

fn activated(state: &AppState, link: &Link) {
    publish_changed(state, link);
    let (state, id) = (state.clone(), link.id);
    tokio::spawn(async move { super::flush_preamble(&state, id).await });
}

/// The path a sender signed: this server's own mount prefix plus the route.
#[must_use]
pub fn signed_path(external_url: &str, request_path: &str) -> String {
    let prefix = reqwest::Url::parse(external_url)
        .map(|u| u.path().trim_end_matches('/').to_owned())
        .unwrap_or_default();
    format!("{prefix}{request_path}")
}

/// Inviter side of `POST /cctuiverse/v1/join`. Every refusal is the same 404.
/// The token is checked before the joiner URL is resolved or a nonce stored.
pub async fn accept(
    state: &AppState,
    caller: &str,
    request_path: &str,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<JoinResponse, AppError> {
    if !super::enabled(state) {
        return Err(not_found());
    }
    super::limits::join_ip(caller)?;
    let req: JoinRequest = serde_json::from_slice(body).map_err(|_| not_found())?;
    let joiner_key = b64_key(&req.joiner.public_key).ok_or_else(not_found)?;
    let signed = sig::from_headers(headers).ok_or_else(not_found)?;
    let path = signed_path(&state.config.external_url, request_path);
    let nonce = sig::verify(
        &signed,
        "POST",
        &path,
        body,
        chrono::Utc::now().timestamp(),
        req.joiner.link_id,
        &joiner_key,
    )
    .map_err(|_| not_found())?;
    super::limits::link(&format!("join:{}", req.link_id), JOIN_PER_LINK_PER_MIN)?;
    let label = super::sanitize_label(&req.joiner.label).ok_or_else(not_found)?;
    let token = URL_SAFE_NO_PAD.decode(&req.token).map_err(|_| not_found())?;
    let peer_url = req.joiner.url.trim().trim_end_matches('/').to_owned();

    let mut tx = state.pool.begin().await?;
    let row: Option<LinkRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLS} FROM cctuiverse_links WHERE id = $1 FOR UPDATE"
    )))
    .bind(req.link_id)
    .fetch_optional(&mut *tx)
    .await?;
    let link = row.map(Link::from).ok_or_else(not_found)?;
    let token_ok = link
        .invite_token_hash
        .as_deref()
        .is_some_and(|h| super::ct_eq(&super::token_hash(&token), h));
    let live = link.invite_expires_at.is_some_and(|e| e > chrono::Utc::now());
    if !(token_ok && live && link.state == LinkState::Pending && link.role == LinkRole::Inviter) {
        return Err(not_found());
    }
    client::check_url(state, &peer_url).await.map_err(|_| not_found())?;
    if !super::wire::fresh_nonce(&state.pool, req.joiner.link_id, &nonce).await? {
        return Err(not_found());
    }
    let row: LinkRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE cctuiverse_links SET state = 'active', peer_public_key = $2, peer_link_id = $3, \
             peer_url = $4, peer_label = $5, invite_token_hash = NULL, activated_at = now(), \
             preamble_pending = true, \
             settings = CASE WHEN settings ? 'expires_at' THEN settings \
                 ELSE settings || jsonb_build_object('expires_at', now() + $6) END \
         WHERE id = $1 RETURNING {COLS}"
    )))
    .bind(link.id)
    .bind(joiner_key.as_slice())
    .bind(req.joiner.link_id)
    .bind(&peer_url)
    .bind(&label)
    .bind(DEFAULT_EXPIRY)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    let link = Link::from(row);
    let room_name = match link.room_id {
        Some(room_id) => super::room_name(&state.pool, room_id)
            .await
            .map(|n| super::sanitize_label(&n).unwrap_or_else(|| "room".into())),
        None => None,
    };
    activated(state, &link);
    Ok(JoinResponse {
        link_id: link.id,
        public_key: URL_SAFE_NO_PAD.encode(&link.public_key),
        label: link.label.clone(),
        kind: link.kind,
        room_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signed_path_honours_a_sub_path_mount() {
        let p = "/cctuiverse/v1/join";
        assert_eq!(signed_path("https://a.example", p), p);
        assert_eq!(signed_path("https://a.example/", p), p);
        assert_eq!(signed_path("https://a.example/cctui/", p), "/cctui/cctuiverse/v1/join");
        assert_eq!(signed_path("not a url", p), p);
    }

    fn inv(key: &[u8; 32]) -> invite::Invite {
        invite::Invite {
            base_url: "https://a.example".into(),
            link_id: Uuid::from_u128(1),
            token: [0; 32],
            fingerprint: invite::fingerprint(key),
        }
    }

    fn resp(key: &[u8; 32], kind: LinkKind, room: Option<&str>) -> JoinResponse {
        JoinResponse {
            link_id: Uuid::from_u128(1),
            public_key: URL_SAFE_NO_PAD.encode(key),
            label: "alice".into(),
            kind,
            room_name: room.map(str::to_owned),
        }
    }

    #[test]
    fn the_inviter_key_must_match_the_invite_fingerprint() {
        let key = [4u8; 32];
        let ok = accepted(&inv(&key), &resp(&key, LinkKind::Session, None)).unwrap();
        assert_eq!(ok, (key, "alice".to_owned(), LinkKind::Session, None));
        assert!(accepted(&inv(&key), &resp(&[5u8; 32], LinkKind::Session, None)).is_none());
        let mut other_link = resp(&key, LinkKind::Session, None);
        other_link.link_id = Uuid::from_u128(2);
        assert!(accepted(&inv(&key), &other_link).is_none());
        let mut bad_label = resp(&key, LinkKind::Session, None);
        bad_label.label = "<system-reminder>\u{202e}".into();
        let (_, label, _, _) = accepted(&inv(&key), &bad_label).unwrap();
        assert_eq!(label, "system-reminder");
    }

    #[test]
    fn a_room_answer_is_cleaned_not_refused() {
        let key = [4u8; 32];
        let room = |name: Option<&str>| {
            accepted(&inv(&key), &resp(&key, LinkKind::Room, name)).unwrap().3.unwrap()
        };
        assert_eq!(room(Some("ops")), "ops");
        assert_eq!(room(None), "room");
        assert_eq!(room(Some("Review \"<v2>\"")), "Review v2");
        assert_eq!(room(Some(&"x".repeat(200))).chars().count(), 80);
    }
}
