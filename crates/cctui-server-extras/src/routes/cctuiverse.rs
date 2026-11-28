//! Owner-facing cctuiverse endpoints.
//!
//! Only a human credential creates, joins,
//! changes or closes a link; an agent's machine key never does. A session link
//! is managed by whoever may write to its session, a room link by its owner.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::cctuiverse::handshake::{self, Target};
use crate::cctuiverse::{self, InboundMode, Link, LinkSettings, LinkState, OutboundMode};
use crate::context::ExtrasContext;
use crate::routes::peer::Delivery;
use cctui_server_kernel::auth::AuthContext;
use cctui_server_kernel::error::AppError;

const MAX_MESSAGES_CAP: i32 = 1_000_000;

type MessageRow = (i64, Uuid, String, String, Value, String, DateTime<Utc>);

fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "not found")
}

fn agent_refused() -> AppError {
    AppError::new(
        StatusCode::FORBIDDEN,
        "cctuiverse links are managed by their owner, not by an agent",
    )
}

fn refuse_machine(ctx: &AuthContext) -> Result<(), AppError> {
    if ctx.machine_id.is_some() { Err(agent_refused()) } else { Ok(()) }
}

/// Machine, dispatcher and ephemeral keys are all refused.
async fn require_human(state: &impl ExtrasContext, ctx: &AuthContext) -> Result<(), AppError> {
    refuse_machine(ctx)?;
    if cctui_server_kernel::auth::is_human_credential(state.pool(), ctx).await? {
        Ok(())
    } else {
        Err(agent_refused())
    }
}

fn require_enabled(state: &impl ExtrasContext) -> Result<(), AppError> {
    if cctuiverse::enabled(state) { Ok(()) } else { Err(not_found()) }
}

fn label(raw: &str) -> Result<String, AppError> {
    cctuiverse::clean_label(raw).ok_or_else(|| {
        AppError::new(
            StatusCode::BAD_REQUEST,
            "label must be 1-80 characters, without control characters, quotes or angle brackets",
        )
    })
}

/// Whoever may write to a session may link it: its owner or an admin, never a
/// read-only share grantee.
async fn writable_session(
    state: &impl ExtrasContext,
    ctx: &AuthContext,
    id: &str,
) -> Result<(), AppError> {
    cctui_server_kernel::authz::authorize_session_write(ctx, id, state.pool()).await.map_err(
        |status| {
            let msg = if status == StatusCode::FORBIDDEN {
                "only the session's owner can link it"
            } else {
                "no such session"
            };
            AppError::new(status, msg)
        },
    )
}

async fn own_room(
    state: &impl ExtrasContext,
    ctx: &AuthContext,
    id: Uuid,
) -> Result<crate::rooms::Room, AppError> {
    crate::rooms::load(state.pool(), id, ctx.user_id)
        .await?
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such room"))
}

async fn own_link(
    state: &impl ExtrasContext,
    ctx: &AuthContext,
    id: Uuid,
) -> Result<Link, AppError> {
    let link = cctuiverse::load(state.pool(), id).await?.ok_or_else(not_found)?;
    if link.user_id == ctx.user_id {
        return Ok(link);
    }
    match link.session_id.as_deref() {
        Some(sid)
            if cctui_server_kernel::authz::authorize_session_write(ctx, sid, state.pool())
                .await
                .is_ok() =>
        {
            Ok(link)
        }
        _ => Err(not_found()),
    }
}

async fn link_json(state: &impl ExtrasContext, link: &Link) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({ "link": cctuiverse::view(state.pool(), link).await? })))
}

/// `GET /api/v1/cctuiverse/config`.
#[allow(clippy::unused_async)]
pub async fn config<S: ExtrasContext>(State(state): State<S>) -> Json<Value> {
    Json(json!({ "enabled": cctuiverse::enabled(&state) }))
}

#[derive(Debug, Deserialize)]
pub struct InviteRequest {
    pub label: String,
}

/// `POST /api/v1/sessions/{id}/cctuiverse/invites`.
pub async fn invite_session<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<String>,
    Json(req): Json<InviteRequest>,
) -> Result<Json<Value>, AppError> {
    require_human(&state, &ctx).await?;
    require_enabled(&state)?;
    writable_session(&state, &ctx, &id).await?;
    let label = label(&req.label)?;
    if cctuiverse::session_state(state.pool(), &id).await == Some("archived") {
        return Err(AppError::new(StatusCode::CONFLICT, "the session is archived"));
    }
    let (link, invite) =
        handshake::create_invite(&state, ctx.user_id, Target::Session(&id), &label).await?;
    Ok(Json(json!({ "link": cctuiverse::view(state.pool(), &link).await?, "invite": invite })))
}

/// `POST /api/v1/rooms/{id}/cctuiverse/invites`.
pub async fn invite_room<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<InviteRequest>,
) -> Result<Json<Value>, AppError> {
    require_human(&state, &ctx).await?;
    require_enabled(&state)?;
    let room = own_room(&state, &ctx, id).await?;
    let label = label(&req.label)?;
    if room.archived {
        return Err(AppError::new(StatusCode::CONFLICT, "the room is archived"));
    }
    let (link, invite) =
        handshake::create_invite(&state, ctx.user_id, Target::Room(id), &label).await?;
    Ok(Json(json!({ "link": cctuiverse::view(state.pool(), &link).await?, "invite": invite })))
}

#[derive(Debug, Deserialize)]
pub struct JoinRequest {
    pub invite: String,
    pub session_id: String,
    pub label: String,
}

/// `POST /api/v1/cctuiverse/join`.
pub async fn join<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<JoinRequest>,
) -> Result<Json<Value>, AppError> {
    require_human(&state, &ctx).await?;
    require_enabled(&state)?;
    writable_session(&state, &ctx, &req.session_id).await?;
    let label = label(&req.label)?;
    if cctuiverse::session_state(state.pool(), &req.session_id).await == Some("archived") {
        return Err(AppError::new(StatusCode::CONFLICT, "the session is archived"));
    }
    let link = handshake::join(&state, ctx.user_id, &req.invite, &req.session_id, &label).await?;
    link_json(&state, &link).await
}

async fn list(
    state: &impl ExtrasContext,
    session: Option<&str>,
    room: Option<Uuid>,
) -> Result<Json<Value>, AppError> {
    let mut views = Vec::new();
    for link in cctuiverse::links_of(state.pool(), session, room).await? {
        views.push(cctuiverse::view(state.pool(), &link).await?);
    }
    Ok(Json(json!({ "links": views })))
}

/// `GET /api/v1/sessions/{id}/cctuiverse/links`.
pub async fn session_links<S: ExtrasContext>(
    State(state): State<S>,
    Path(id): Path<String>,
) -> Result<Json<Value>, AppError> {
    require_enabled(&state)?;
    list(&state, Some(&id), None).await
}

/// `GET /api/v1/rooms/{id}/cctuiverse/links`.
pub async fn room_links<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require_enabled(&state)?;
    own_room(&state, &ctx, id).await?;
    list(&state, None, Some(id)).await
}

/// A nullable PATCH field: absent keeps the value, `null` clears it.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Nullable<T> {
    #[default]
    Absent,
    Null,
    Value(T),
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Nullable<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(Option::<T>::deserialize(d)?.map_or(Self::Null, Self::Value))
    }
}

impl<T> Nullable<T> {
    fn apply(self, slot: &mut Option<T>) {
        match self {
            Self::Absent => {}
            Self::Null => *slot = None,
            Self::Value(v) => *slot = Some(v),
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SettingsPatch {
    #[serde(default)]
    pub inbound: Option<InboundMode>,
    #[serde(default)]
    pub outbound: Option<OutboundMode>,
    #[serde(default)]
    pub review_outbound: Option<bool>,
    #[serde(default)]
    pub share_transcript: Option<bool>,
    #[serde(default)]
    pub expires_at: Nullable<DateTime<Utc>>,
    #[serde(default)]
    pub max_messages: Nullable<i32>,
}

impl SettingsPatch {
    pub fn apply(self, mut s: LinkSettings) -> Result<LinkSettings, String> {
        if let Nullable::Value(n) = self.max_messages
            && !(0..=MAX_MESSAGES_CAP).contains(&n)
        {
            return Err(format!("max_messages must be 0..={MAX_MESSAGES_CAP}"));
        }
        if let Some(v) = self.inbound {
            s.inbound = v;
        }
        if let Some(v) = self.outbound {
            s.outbound = v;
        }
        if let Some(v) = self.review_outbound {
            s.review_outbound = v;
        }
        if let Some(v) = self.share_transcript {
            s.share_transcript = v;
        }
        self.expires_at.apply(&mut s.expires_at);
        self.max_messages.apply(&mut s.max_messages);
        Ok(s)
    }
}

/// `PATCH /api/v1/cctuiverse/links/{id}`.
pub async fn update<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(patch): Json<SettingsPatch>,
) -> Result<Json<Value>, AppError> {
    require_human(&state, &ctx).await?;
    require_enabled(&state)?;
    let link = own_link(&state, &ctx, id).await?;
    if link.state == LinkState::Closed {
        return Err(AppError::new(StatusCode::CONFLICT, "the link is closed"));
    }
    if link.kind == cctuiverse::LinkKind::Room
        && link.role == cctuiverse::LinkRole::Inviter
        && patch.inbound == Some(InboundMode::Hold)
    {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            "a room's own links cannot hold: posts go to every member",
        ));
    }
    let settings = patch
        .apply(link.settings.clone())
        .map_err(|e| AppError::new(StatusCode::BAD_REQUEST, e))?;
    sqlx::query("UPDATE cctuiverse_links SET settings = $2 WHERE id = $1")
        .bind(id)
        .bind(serde_json::to_value(&settings)?)
        .execute(state.pool())
        .await?;
    let link = own_link(&state, &ctx, id).await?;
    cctuiverse::publish_changed(&state, &link);
    link_json(&state, &link).await
}

/// `POST /api/v1/cctuiverse/links/{id}/close`.
pub async fn close<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<Json<Value>, AppError> {
    require_human(&state, &ctx).await?;
    let link = own_link(&state, &ctx, id).await?;
    let link = cctuiverse::close(&state, &link, cctuiverse::CloseReason::Owner).await?;
    link_json(&state, &link).await
}

#[derive(Debug, Deserialize)]
pub struct MessagesQuery {
    #[serde(default)]
    pub status: Option<String>,
}

/// `GET /api/v1/cctuiverse/links/{id}/messages`.
pub async fn messages<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Query(q): Query<MessagesQuery>,
) -> Result<Json<Value>, AppError> {
    let link = own_link(&state, &ctx, id).await?;
    let statuses: Vec<&str> = match q.status.as_deref() {
        None | Some("") => vec!["held", "review"],
        Some(s @ ("held" | "review")) => vec![s],
        Some(_) => {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "status must be held or review"));
        }
    };
    let rows: Vec<MessageRow> = sqlx::query_as(
        "SELECT id, message_id, direction, kind, body, status, created_at FROM cctuiverse_messages \
         WHERE link_id = $1 AND status = ANY($2) ORDER BY id LIMIT 500",
    )
    .bind(link.id)
    .bind(&statuses)
    .fetch_all(state.pool())
    .await?;
    let messages: Vec<_> = rows
        .into_iter()
        .map(|(id, message_id, direction, kind, body, status, created_at)| {
            cctui_proto::api::cctuiverse::CctuiverseMessageView {
                id,
                message_id,
                direction,
                kind,
                text: body["text"].as_str().unwrap_or_default().to_owned(),
                status,
                created_at,
            }
        })
        .collect();
    Ok(Json(json!({ "messages": messages })))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Release,
    Drop,
    Approve,
}

async fn decide(
    state: &impl ExtrasContext,
    ctx: &AuthContext,
    id: Uuid,
    msg: i64,
    decision: Decision,
) -> Result<Json<Value>, AppError> {
    require_human(state, ctx).await?;
    let link = own_link(state, ctx, id).await?;
    let row: Option<(String, String, String, Value)> = sqlx::query_as(
        "SELECT direction, kind, status, body FROM cctuiverse_messages WHERE id = $1 AND link_id = $2",
    )
    .bind(msg)
    .bind(link.id)
    .fetch_optional(state.pool())
    .await?;
    let (direction, kind, status, body) = row.ok_or_else(not_found)?;
    let conflict = |m: &str| AppError::new(StatusCode::CONFLICT, m.to_owned());
    let mut outcome = json!(null);
    match (decision, direction.as_str(), status.as_str()) {
        (Decision::Drop, _, "held" | "review") => {
            set(state.pool(), msg, &status, "dropped").await?;
        }
        (Decision::Release, "in", "held") => release_held(state, &link, msg, &kind, &body).await?,
        (Decision::Approve, "out", "review") => {
            if !link.usable() {
                return Err(conflict("the link is closed or expired"));
            }
            sqlx::query(
                "UPDATE cctuiverse_messages SET status = 'queued', \
                     next_attempt_at = now() + interval '1 minute', first_queued_at = now() \
                 WHERE id = $1 AND status = 'review'",
            )
            .bind(msg)
            .execute(state.pool())
            .await?;
            outcome = match cctuiverse::outbox::attempt(state, &link, msg).await {
                cctuiverse::SendOutcome::Delivered => json!("delivered"),
                cctuiverse::SendOutcome::Refused(why) => json!({ "refused": why }),
                cctuiverse::SendOutcome::Queued | cctuiverse::SendOutcome::AwaitingReview => {
                    json!("queued")
                }
            };
        }
        _ => return Err(conflict("the message is not waiting for that decision")),
    }
    let link = own_link(state, ctx, id).await?;
    cctuiverse::publish_changed(state, &link);
    Ok(Json(json!({ "link": cctuiverse::view(state.pool(), &link).await?, "outcome": outcome })))
}

/// Delivers a held inbound message while holding its row, so a concurrent
/// release or drop waits for the outcome and then finds it handled.
async fn release_held(
    state: &impl ExtrasContext,
    link: &Link,
    msg: i64,
    kind: &str,
    body: &Value,
) -> Result<(), AppError> {
    let conflict = |m: &str| AppError::new(StatusCode::CONFLICT, m.to_owned());
    if link.state == LinkState::Closed {
        return Err(conflict("the link is closed"));
    }
    let sid = link.session_id.as_deref().ok_or_else(not_found)?;
    let turn = cctuiverse::wire::inbound_turn(link, kind, body).ok_or_else(not_found)?;
    let mut tx = state.pool().begin().await?;
    if !claim_held(&mut tx, msg).await? {
        return Err(conflict(ALREADY_HANDLED));
    }
    match cctuiverse::deliver_local(state, sid, turn).await {
        Delivery::Delivered => {
            set(&mut *tx, msg, "held", "released").await?;
            tx.commit().await?;
            Ok(())
        }
        Delivery::Offline => {
            Err(AppError::new(StatusCode::SERVICE_UNAVAILABLE, "the session is offline"))
        }
        Delivery::Archived | Delivery::Ended => Err(conflict("the session has ended")),
    }
}

/// Locks the message row for the transaction; `true` iff it is still held.
async fn claim_held(conn: &mut sqlx::PgConnection, msg: i64) -> Result<bool, sqlx::Error> {
    let status: Option<String> =
        sqlx::query_scalar("SELECT status FROM cctuiverse_messages WHERE id = $1 FOR UPDATE")
            .bind(msg)
            .fetch_optional(conn)
            .await?;
    Ok(status.as_deref() == Some("held"))
}

const ALREADY_HANDLED: &str = "the message was already handled";

async fn set<'e>(
    db: impl sqlx::PgExecutor<'e>,
    msg: i64,
    from: &str,
    to: &str,
) -> Result<(), AppError> {
    let r = sqlx::query(
        "UPDATE cctuiverse_messages SET status = $3, \
             delivered_at = CASE WHEN $3 = 'released' THEN now() ELSE delivered_at END \
         WHERE id = $1 AND status = $2",
    )
    .bind(msg)
    .bind(from)
    .bind(to)
    .execute(db)
    .await?;
    if r.rows_affected() == 0 {
        return Err(AppError::new(StatusCode::CONFLICT, ALREADY_HANDLED));
    }
    Ok(())
}

/// `POST /api/v1/cctuiverse/links/{id}/messages/{msg}/release`.
pub async fn release<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, msg)): Path<(Uuid, i64)>,
) -> Result<Json<Value>, AppError> {
    decide(&state, &ctx, id, msg, Decision::Release).await
}

/// `POST /api/v1/cctuiverse/links/{id}/messages/{msg}/drop`.
pub async fn drop_message<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, msg)): Path<(Uuid, i64)>,
) -> Result<Json<Value>, AppError> {
    decide(&state, &ctx, id, msg, Decision::Drop).await
}

/// `POST /api/v1/cctuiverse/links/{id}/messages/{msg}/approve`.
pub async fn approve<S: ExtrasContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path((id, msg)): Path<(Uuid, i64)>,
) -> Result<Json<Value>, AppError> {
    decide(&state, &ctx, id, msg, Decision::Approve).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(v: Value) -> SettingsPatch {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_partial_patch_changes_only_what_it_names() {
        let base = LinkSettings {
            expires_at: Some("2030-01-01T00:00:00Z".parse().unwrap()),
            max_messages: Some(5),
            ..LinkSettings::default()
        };
        let s = patch(json!({ "inbound": "hold" })).apply(base.clone()).unwrap();
        assert_eq!(s.inbound, InboundMode::Hold);
        assert_eq!(s.expires_at, base.expires_at);
        assert_eq!(s.max_messages, Some(5));
        assert_eq!(s.outbound, OutboundMode::Tool);
    }

    #[test]
    fn an_explicit_null_clears_and_absence_keeps() {
        let base = LinkSettings {
            expires_at: Some("2030-01-01T00:00:00Z".parse().unwrap()),
            max_messages: Some(5),
            ..LinkSettings::default()
        };
        let s =
            patch(json!({ "expires_at": null, "max_messages": null })).apply(base.clone()).unwrap();
        assert_eq!((s.expires_at, s.max_messages), (None, None));
        let s = patch(json!({})).apply(base.clone()).unwrap();
        assert_eq!(s, base);
        let s =
            patch(json!({ "outbound": "both", "share_transcript": true, "review_outbound": true }))
                .apply(base)
                .unwrap();
        assert!(s.share_transcript && s.review_outbound);
        assert_eq!(s.outbound, OutboundMode::Both);
    }

    #[test]
    fn bad_patches_are_refused() {
        assert!(patch(json!({ "max_messages": -1 })).apply(LinkSettings::default()).is_err());
        assert!(serde_json::from_value::<SettingsPatch>(json!({ "inbound": "steer" })).is_err());
        assert!(serde_json::from_value::<SettingsPatch>(json!({ "unknown": 1 })).is_err());
    }

    #[test]
    fn machine_keys_cannot_manage_links() {
        let ctx = |machine: Option<Uuid>| AuthContext {
            user_id: Uuid::nil(),
            key_id: Uuid::nil(),
            machine_id: machine,
            scopes: std::collections::BTreeSet::new(),
        };
        assert!(refuse_machine(&ctx(None)).is_ok());
        let err = refuse_machine(&ctx(Some(Uuid::nil()))).unwrap_err();
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
    }

    async fn held_message(pool: &sqlx::PgPool, name: &str) -> (Uuid, String, i64) {
        let (uid, sid) = cctui_server_kernel::test_support::seed_session(pool, name).await;
        let link = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO cctuiverse_links (id, user_id, session_id, kind, role, state, label, \
             public_key, envelope_nonce) \
             VALUES ($1, $2, $3, 'session', 'inviter', 'active', 'l', '\\x00', 'n')",
        )
        .bind(link)
        .bind(uid)
        .bind(&sid)
        .execute(pool)
        .await
        .unwrap();
        let msg: i64 = sqlx::query_scalar(
            "INSERT INTO cctuiverse_messages (link_id, message_id, direction, kind, body, status) \
             VALUES ($1, $2, 'in', 'direct', '{}', 'held') RETURNING id",
        )
        .bind(link)
        .bind(Uuid::new_v4())
        .fetch_one(pool)
        .await
        .unwrap();
        (uid, sid, msg)
    }

    async fn drop_seed(pool: &sqlx::PgPool, uid: Uuid) {
        for sql in [
            "DELETE FROM cctuiverse_links WHERE user_id = $1",
            "DELETE FROM sessions WHERE user_id = $1",
            "DELETE FROM machines WHERE user_id = $1",
            "DELETE FROM users WHERE id = $1",
        ] {
            sqlx::query(sql).bind(uid).execute(pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_released_message_is_claimed_before_delivery() {
        let name = "cctuiverse_release_claims_first";
        let Some(url) = cctui_server_kernel::test_support::db_url(name) else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(3).connect(&url).await.unwrap();
        let (uid, _, msg) = held_message(&pool, name).await;

        let mut first = pool.begin().await.unwrap();
        assert!(claim_held(&mut first, msg).await.unwrap());
        let second = tokio::spawn({
            let pool = pool.clone();
            async move {
                let mut tx = pool.begin().await.unwrap();
                claim_held(&mut tx, msg).await.unwrap()
            }
        });
        let dropper = tokio::spawn({
            let pool = pool.clone();
            async move { set(&pool, msg, "held", "dropped").await.map_err(|e| e.status()) }
        });
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert!(!second.is_finished(), "a second release must wait for the first");
        assert!(!dropper.is_finished(), "a drop must wait for the release");
        set(&mut *first, msg, "held", "released").await.unwrap();
        first.commit().await.unwrap();

        assert!(!second.await.unwrap(), "the second release finds it handled");
        assert_eq!(dropper.await.unwrap(), Err(StatusCode::CONFLICT));
        let status: String =
            sqlx::query_scalar("SELECT status FROM cctuiverse_messages WHERE id = $1")
                .bind(msg)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(status, "released");
        drop_seed(&pool, uid).await;
    }

    #[tokio::test]
    async fn a_failed_release_leaves_the_message_held() {
        let name = "cctuiverse_failed_release_rolls_back";
        let Some(url) = cctui_server_kernel::test_support::db_url(name) else {
            return;
        };
        let pool =
            sqlx::postgres::PgPoolOptions::new().max_connections(2).connect(&url).await.unwrap();
        let (uid, _, msg) = held_message(&pool, name).await;
        {
            let mut tx = pool.begin().await.unwrap();
            assert!(claim_held(&mut tx, msg).await.unwrap());
        }
        let mut tx = pool.begin().await.unwrap();
        assert!(claim_held(&mut tx, msg).await.unwrap(), "a dropped claim releases the row");
        tx.rollback().await.unwrap();
        set(&pool, msg, "held", "dropped").await.unwrap();
        drop_seed(&pool, uid).await;
    }
}
