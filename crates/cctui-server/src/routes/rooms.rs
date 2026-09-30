//! Room routes: the small REST surface the webui's room picker needs, the
//! daemon-authenticated `CctuiRoom` endpoint, and the writer for
//! `session_peer_shares`.
//!
//! There is no room page and no human timeline: a room is a field on a session
//! plus a permission boundary, so the only human operations are naming a room
//! and moving sessions in and out of it.
//!
//! Rooms are owned by a user, and there is no `ResourceKind::Room`: every query
//! here carries `user_id = $owner` itself rather than relying on a route guard,
//! so an unknown id and another owner's id answer identically.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::rooms::{self, Member, Room};
use crate::state::AppState;

/// Rooms a user may hold at once. A room is cheap, but the fan-out is not.
const MAX_ROOMS: i64 = 50;
/// Members per room. Every post costs one turn per member.
const MAX_MEMBERS: usize = 16;
const TIMELINE_PAGE: i64 = 200;

fn not_found() -> AppError {
    AppError::new(StatusCode::NOT_FOUND, "no such room")
}

async fn owned(state: &AppState, id: Uuid, owner: Uuid) -> Result<Room, AppError> {
    rooms::load(&state.pool, id, owner).await?.ok_or_else(not_found)
}

/// Confirm `session_id` belongs to `owner` and return its display row, so a
/// room can never be pointed at somebody else's session.
async fn own_session(
    state: &AppState,
    session_id: &str,
    owner: Uuid,
) -> Result<crate::peer_policy::SessionNode, AppError> {
    crate::peer_policy::load_node(&state.pool, session_id)
        .await?
        .filter(|n| n.user_id == Some(owner))
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such session"))
}

/// `GET /api/v1/rooms`.
pub async fn list_rooms(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<Value>, AppError> {
    Ok(Json(json!({ "rooms": rooms::list(&state.pool, ctx.user_id).await? })))
}

#[derive(Debug, serde::Deserialize)]
pub struct UpdateRoomRequest {
    pub name: Option<String>,
    pub archived: Option<bool>,
}

/// `PATCH /api/v1/rooms/{id}` — rename, archive or unarchive.
pub async fn update_room(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateRoomRequest>,
) -> Result<Json<Value>, AppError> {
    owned(&state, id, ctx.user_id).await?;
    if let Some(name) = req.name.as_deref().map(str::trim) {
        if name.is_empty() {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "room name is required"));
        }
        sqlx::query("UPDATE rooms SET name = $2 WHERE id = $1")
            .bind(id)
            .bind(name)
            .execute(&state.pool)
            .await?;
    }
    if let Some(archived) = req.archived {
        let at = archived.then(chrono::Utc::now);
        sqlx::query("UPDATE rooms SET archived_at = $2 WHERE id = $1")
            .bind(id)
            .bind(at)
            .execute(&state.pool)
            .await?;
    }
    state
        .bus
        .publish_server(cctui_proto::ws::ServerEvent::RoomMembers { room_id: id, user_id: ctx.user_id });
    Ok(Json(json!(owned(&state, id, ctx.user_id).await?)))
}

/// `DELETE /api/v1/rooms/{id}` — drops the timeline with it (cascade).
pub async fn delete_room(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, AppError> {
    owned(&state, id, ctx.user_id).await?;
    sqlx::query("DELETE FROM rooms WHERE id = $1 AND user_id = $2")
        .bind(id)
        .bind(ctx.user_id)
        .execute(&state.pool)
        .await?;
    state
        .bus
        .publish_server(cctui_proto::ws::ServerEvent::RoomMembers { room_id: id, user_id: ctx.user_id });
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, serde::Deserialize)]
pub struct SetRoomRequest {
    /// An existing room id, or — when `name` is given instead — the name to
    /// create-or-reuse. Exactly one of the two is required.
    #[serde(default)]
    pub room_id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
}

/// Get-or-create a room by name for `owner`, case-insensitively, reviving an
/// archived one rather than colliding with it.
async fn room_by_name(state: &AppState, owner: Uuid, name: &str) -> Result<Uuid, AppError> {
    let live: i64 =
        sqlx::query_scalar("SELECT count(*) FROM rooms WHERE user_id = $1 AND archived_at IS NULL")
            .bind(owner)
            .fetch_one(&state.pool)
            .await?;
    if live >= MAX_ROOMS {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            format!("you already have {MAX_ROOMS} live rooms; archive one first"),
        ));
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO rooms (user_id, name) VALUES ($1, $2) \
         ON CONFLICT (user_id, lower(name)) \
           DO UPDATE SET name = EXCLUDED.name, archived_at = NULL \
         RETURNING id",
    )
    .bind(owner)
    .bind(name)
    .fetch_one(&state.pool)
    .await?;
    Ok(id)
}

/// `PUT /api/v1/sessions/{id}/room` — put this session in a room, by id or by
/// name, moving it out of whatever room it was in.
pub async fn set_session_room(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<SetRoomRequest>,
) -> Result<Json<Value>, AppError> {
    own_session(&state, &session_id, ctx.user_id).await?;
    let id = match (req.room_id.as_deref().map(str::trim), req.name.as_deref().map(str::trim)) {
        (Some(raw), _) if !raw.is_empty() => Uuid::parse_str(raw)
            .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "room_id is not a room id"))?,
        (_, Some(name)) if !name.is_empty() => room_by_name(&state, ctx.user_id, name).await?,
        _ => {
            return Err(AppError::new(StatusCode::BAD_REQUEST, "room_id or name is required"));
        }
    };
    let room = owned(&state, id, ctx.user_id).await?;
    if room.archived {
        return Err(AppError::new(StatusCode::CONFLICT, "this room is archived"));
    }
    if room.members.len() >= MAX_MEMBERS
        && !room.members.iter().any(|m| m.session_id == session_id)
    {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            format!("this room already holds {MAX_MEMBERS} sessions"),
        ));
    }
    rooms::set_room(&state, &room, ctx.user_id, &session_id).await?;
    Ok(Json(json!({ "room_id": room.id, "name": room.name })))
}

/// `DELETE /api/v1/sessions/{id}/room` — take this session out of its room.
pub async fn clear_session_room(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, AppError> {
    own_session(&state, &session_id, ctx.user_id).await?;
    rooms::clear_room(&state, ctx.user_id, &session_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- session_peer_shares writer ---

#[derive(Debug, serde::Deserialize)]
pub struct PeerShareRequest {
    pub peer_session_id: String,
}

/// `POST /api/v1/sessions/{id}/peer-shares` — let two otherwise unrelated
/// sessions of the same owner address each other. Symmetric: one row, both
/// directions, matching what [`crate::peer_policy`] reads.
pub async fn create_peer_share(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Json(req): Json<PeerShareRequest>,
) -> Result<(StatusCode, Json<Value>), AppError> {
    let peer = req.peer_session_id.trim();
    if peer.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "peer_session_id is required"));
    }
    if peer == session_id {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "a session cannot be shared with itself"));
    }
    own_session(&state, &session_id, ctx.user_id).await?;
    own_session(&state, peer, ctx.user_id).await?;
    // Re-granting a revoked pair must revive it, not collide with the dead row:
    // the unique index only covers live grants.
    sqlx::query(
        "INSERT INTO session_peer_shares (session_id, peer_session_id, granted_by) \
         VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
    )
    .bind(&session_id)
    .bind(peer)
    .bind(ctx.user_id)
    .execute(&state.pool)
    .await?;
    Ok((StatusCode::CREATED, Json(json!({ "session_id": session_id, "peer_session_id": peer }))))
}

/// `DELETE /api/v1/sessions/{id}/peer-shares/{peer_session_id}` — revoke in
/// either direction, since the grant is symmetric.
pub async fn revoke_peer_share(
    State(state): State<AppState>,
    Path((session_id, peer)): Path<(String, String)>,
) -> Result<StatusCode, AppError> {
    sqlx::query(
        "UPDATE session_peer_shares SET revoked_at = now() \
         WHERE revoked_at IS NULL \
           AND ((session_id = $1 AND peer_session_id = $2) \
             OR (session_id = $2 AND peer_session_id = $1))",
    )
    .bind(&session_id)
    .bind(&peer)
    .execute(&state.pool)
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/sessions/{id}/peer-shares` — who this session is shared with.
pub async fn list_peer_shares(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Value>, AppError> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "SELECT s.id, s.session_name FROM session_peer_shares p \
           JOIN sessions s ON s.id = CASE WHEN p.session_id = $1 THEN p.peer_session_id \
                                          ELSE p.session_id END \
          WHERE p.revoked_at IS NULL AND (p.session_id = $1 OR p.peer_session_id = $1) \
          ORDER BY s.id",
    )
    .bind(&session_id)
    .fetch_all(&state.pool)
    .await?;
    let out: Vec<Value> = rows
        .into_iter()
        .map(|(id, name)| json!({ "session_id": id, "name": name }))
        .collect();
    Ok(Json(json!({ "shares": out })))
}

// --- daemon surface for the CctuiRoom tool ---

/// The room a tool call means: the one it named, or the caller's own. A session
/// is in at most one room, so there is never anything to disambiguate.
async fn resolve_room(
    state: &AppState,
    caller: &str,
    owner: Uuid,
    named: Option<&str>,
) -> Result<Room, AppError> {
    if let Some(raw) = named.map(str::trim).filter(|r| !r.is_empty()) {
        let id = Uuid::parse_str(raw)
            .map_err(|_| AppError::new(StatusCode::BAD_REQUEST, "room_id is not a room id"))?;
        let room = owned(state, id, owner).await?;
        if !room.members.iter().any(|m| m.session_id == caller) {
            return Err(rooms::PostRefusal::NotAMember.into());
        }
        return Ok(room);
    }
    let Some((id, _)) = rooms::room_of_session(&state.pool, caller).await? else {
        return Err(AppError::new(
            StatusCode::NOT_FOUND,
            "this session is not in a room, so there is nowhere to post. A human puts a session \
             in a room from the cctui UI.",
        ));
    };
    owned(state, id, owner).await
}

/// `POST /api/v1/daemon/sessions/{id}/room` — the server side of `CctuiRoom`.
pub async fn room_tool(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::RoomToolRequest>,
) -> Result<Json<Value>, AppError> {
    let owner = crate::routes::spawn_child::machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    own_session(&state, &session_id, owner).await?;
    let room = resolve_room(&state, &session_id, owner, req.room_id.as_deref()).await?;
    let me: Option<Member> = room.members.iter().find(|m| m.session_id == session_id).cloned();

    match req.action.trim().to_ascii_lowercase().as_str() {
        "post" => {
            let body = req.message.as_deref().unwrap_or_default();
            let sender = me.as_ref();
            let message = rooms::post(&state, &room, sender, body).await?;
            let recipients = room
                .members
                .iter()
                .filter(|m| m.session_id != session_id)
                .count();
            Ok(Json(json!({
                "room": room.name,
                "room_id": room.id,
                "seq": message.seq,
                "recipients": recipients,
            })))
        }
        "peek" => {
            let seen = me.as_ref().map_or(0, |m| m.last_delivered_seq);
            let messages = rooms::timeline(&state.pool, room.id, None, TIMELINE_PAGE).await?;
            Ok(Json(json!({
                "room": room.name,
                "room_id": room.id,
                "last_delivered_seq": seen,
                "messages": messages,
            })))
        }
        "members" => Ok(Json(json!({
            "room": room.name,
            "room_id": room.id,
            "members": room.members,
        }))),
        other => Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("unknown action {other:?}; use \"post\", \"peek\" or \"members\""),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rooms::PostRefusal;

    /// DB-gated: the whole room lifecycle — create, seed members across two
    /// machines, post, fan out, replay in order after an offline gap, the loop
    /// guard, and the policy relation rooms confer.
    #[tokio::test]
    async fn a_room_fans_out_in_order_and_never_echoes_its_sender() {
        let Some(url) = crate::routes::gateway::test_db_url("rooms_fanout") else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let machine_a = Uuid::new_v4();
        let machine_b = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'rooms-test', $2)")
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
        // A claude session on machine A, a codex session on machine B, and an
        // unrelated session that is in no room.
        let a = Uuid::new_v4().to_string();
        let b = Uuid::new_v4().to_string();
        let outsider = Uuid::new_v4().to_string();
        for (id, m, adapter) in
            [(&a, machine_a, "claude-code"), (&b, machine_b, "codex"), (&outsider, machine_a, "codex")]
        {
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, \
                 adapter_id, session_name, status) \
                 VALUES ($1, $2, '/w', $3, $4, $5, $6, 'active')",
            )
            .bind(id)
            .bind(m.to_string())
            .bind(uid)
            .bind(m)
            .bind(adapter)
            .bind(format!("name-{id}"))
            .execute(&pool)
            .await
            .expect("seed session");
        }
        let room_id: Uuid = sqlx::query_scalar(
            "INSERT INTO rooms (user_id, name) VALUES ($1, 'wave 23') RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .expect("seed room");
        // A room is a field: putting a session in one is an UPDATE.
        for id in [&a, &b] {
            sqlx::query("UPDATE sessions SET room_id = $1 WHERE id = $2")
                .bind(room_id)
                .bind(id)
                .execute(&pool)
                .await
                .expect("seed room_id");
        }

        let room = rooms::load(&pool, room_id, uid).await.unwrap().expect("room");
        assert_eq!(room.members.len(), 2);
        assert_eq!(room.name, "wave 23");
        assert!(rooms::load(&pool, room_id, Uuid::new_v4()).await.unwrap().is_none(),
            "another owner must not see the room");

        // Room membership authorises the pair for the peer tools, and leaves the
        // outsider refused.
        assert_eq!(
            crate::peer_policy::authorize(&pool, &a, &b, uid).await.unwrap().0,
            crate::peer_policy::Relation::Room,
        );
        assert_eq!(
            crate::peer_policy::authorize(&pool, &a, &outsider, uid).await.unwrap_err(),
            crate::peer_policy::Refusal::Unrelated,
        );
        let roster: Vec<crate::peer_policy::RosterRow> =
            sqlx::query_as(crate::peer_policy::ROSTER_SQL)
                .bind(&a)
                .fetch_all(&pool)
                .await
                .expect("roster");
        assert!(
            roster.iter().any(|r| r.0 == b && r.5 == "room"),
            "a room member must appear on the roster as `room`: {roster:?}"
        );

        // Two posts from A while B is offline, written the way `post` does.
        for (n, body) in [(1_i64, "first"), (2, "second")] {
            let seq: i64 = sqlx::query_scalar(
                "UPDATE rooms SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq",
            )
            .bind(room_id)
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(seq, n);
            sqlx::query(
                "INSERT INTO room_messages (room_id, seq, sender_session_id, sender_label, body) \
                 VALUES ($1, $2, $3, 'lane a (claude-code on box-a)', $4)",
            )
            .bind(room_id)
            .bind(seq)
            .bind(&a)
            .bind(body)
            .execute(&pool)
            .await
            .unwrap();
        }

        let pend = rooms::pending(&pool, Some(room_id), 100).await.expect("pending");
        let for_b = pend.iter().find(|p| p.session_id == b).expect("b is owed both posts");
        let for_a = pend.iter().find(|p| p.session_id == a).expect("a's cursor also trails");

        // The loop guard: A is owed nothing, because both posts are its own.
        assert!(
            rooms::pending_for(&pool, for_a).await.unwrap().is_empty(),
            "a sender must never be handed back its own post"
        );
        let owed = rooms::pending_for(&pool, for_b).await.unwrap();
        assert_eq!(owed.len(), 2, "an offline member is owed both posts");
        assert_eq!(
            owed.iter().map(|m| m.body.as_str()).collect::<Vec<_>>(),
            vec!["first", "second"],
            "replay must be in seq order"
        );
        assert_eq!(owed.iter().map(|m| m.seq).collect::<Vec<_>>(), vec![1, 2]);

        // Advancing A past its own posts stops it being selected forever.
        rooms::advance_to(&pool, for_a, None).await.unwrap();
        let after = rooms::pending(&pool, Some(room_id), 100).await.unwrap();
        assert!(
            !after.iter().any(|p| p.session_id == a),
            "a sender's cursor must advance past its own posts"
        );

        // B receives them, in one turn, and is then up to date.
        rooms::advance_to(&pool, for_b, Some(2)).await.unwrap();
        let settled = rooms::pending(&pool, Some(room_id), 100).await.unwrap();
        assert!(settled.is_empty(), "{settled:?}");

        // A third post while B is up to date makes it pending again with only
        // the new message.
        let seq: i64 = sqlx::query_scalar(
            "UPDATE rooms SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq",
        )
        .bind(room_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO room_messages (room_id, seq, sender_session_id, sender_label, body) \
             VALUES ($1, $2, NULL, 'you (human)', 'from the human')",
        )
        .bind(room_id)
        .bind(seq)
        .execute(&pool)
        .await
        .unwrap();
        let again = rooms::pending(&pool, Some(room_id), 100).await.unwrap();
        let for_b = again.iter().find(|p| p.session_id == b).expect("b is owed the human post");
        let owed = rooms::pending_for(&pool, for_b).await.unwrap();
        assert_eq!(owed.len(), 1);
        assert_eq!(owed[0].body, "from the human");
        assert!(owed[0].sender_session_id.is_none(), "a human post has no sender session");
        let for_a = again.iter().find(|p| p.session_id == a).expect("a is owed it too");
        assert_eq!(
            rooms::pending_for(&pool, for_a).await.unwrap().len(),
            1,
            "the human's post reaches the session that posted earlier"
        );

        // What `CctuiRoom peek` reads, and its `after=` cursor.
        let all = rooms::timeline(&pool, room_id, None, 200).await.unwrap();
        assert_eq!(all.len(), 3);
        let delta = rooms::timeline(&pool, room_id, Some(2), 200).await.unwrap();
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[0].seq, 3);

        // An archived room stops fanning out and refuses new posts.
        sqlx::query("UPDATE rooms SET archived_at = now() WHERE id = $1")
            .bind(room_id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            rooms::pending(&pool, Some(room_id), 100).await.unwrap().is_empty(),
            "an archived room must not fan out"
        );
        let archived = rooms::load(&pool, room_id, uid).await.unwrap().unwrap();
        assert!(archived.archived);
        assert_eq!(rooms::check_sender(&archived, Some(&a)), Err(PostRefusal::Archived));
        assert_eq!(
            crate::peer_policy::authorize(&pool, &a, &b, uid).await.unwrap_err(),
            crate::peer_policy::Refusal::Unrelated,
            "an archived room stops authorising its members",
        );

        // A session is in at most one room: moving it to a second one takes it
        // out of the first, because the room is a single column.
        sqlx::query("UPDATE rooms SET archived_at = NULL WHERE id = $1")
            .bind(room_id)
            .execute(&pool)
            .await
            .unwrap();
        let other: Uuid = sqlx::query_scalar(
            "INSERT INTO rooms (user_id, name) VALUES ($1, 'wave 24') RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("UPDATE sessions SET room_id = $1 WHERE id = $2")
            .bind(other)
            .bind(&b)
            .execute(&pool)
            .await
            .unwrap();
        let first = rooms::load(&pool, room_id, uid).await.unwrap().unwrap();
        assert_eq!(
            first.members.iter().map(|m| m.session_id.clone()).collect::<Vec<_>>(),
            vec![a.clone()],
            "moving a session to another room must remove it from the first"
        );
        assert_eq!(rooms::room_of_session(&pool, &b).await.unwrap().map(|r| r.0), Some(other));
        assert_eq!(
            crate::peer_policy::authorize(&pool, &a, &b, uid).await.unwrap_err(),
            crate::peer_policy::Refusal::Unrelated,
            "two different rooms are not a shared room",
        );

        // Deleting a room releases its sessions rather than deleting them.
        sqlx::query("DELETE FROM rooms WHERE id = $1").bind(other).execute(&pool).await.unwrap();
        assert!(rooms::room_of_session(&pool, &b).await.unwrap().is_none());
        let still_there: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE id = $1")
            .bind(&b)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(still_there, 1, "ON DELETE SET NULL, not CASCADE");

        sqlx::query("DELETE FROM rooms WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// DB-gated: `session_peer_shares` now has a writer, and the policy sees
    /// what it writes.
    #[tokio::test]
    async fn the_share_writer_grants_and_revokes_symmetrically() {
        let Some(url) = crate::routes::gateway::test_db_url("rooms_share_writer") else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'share-test', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        let one = Uuid::new_v4().to_string();
        let two = Uuid::new_v4().to_string();
        for id in [&one, &two] {
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, \
                 adapter_id, status) VALUES ($1, $2, '/w', $3, $4, 'codex', 'active')",
            )
            .bind(id)
            .bind(machine.to_string())
            .bind(uid)
            .bind(machine)
            .execute(&pool)
            .await
            .expect("seed session");
        }
        assert_eq!(
            crate::peer_policy::authorize(&pool, &one, &two, uid).await.unwrap_err(),
            crate::peer_policy::Refusal::Unrelated,
        );
        sqlx::query(
            "INSERT INTO session_peer_shares (session_id, peer_session_id, granted_by) \
             VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        )
        .bind(&one)
        .bind(&two)
        .bind(uid)
        .execute(&pool)
        .await
        .expect("grant");
        assert_eq!(
            crate::peer_policy::authorize(&pool, &two, &one, uid).await.unwrap().0,
            crate::peer_policy::Relation::Shared,
            "the grant works in the direction it was not written in",
        );
        sqlx::query(
            "UPDATE session_peer_shares SET revoked_at = now() WHERE revoked_at IS NULL \
               AND ((session_id = $1 AND peer_session_id = $2) \
                 OR (session_id = $2 AND peer_session_id = $1))",
        )
        .bind(&two)
        .bind(&one)
        .execute(&pool)
        .await
        .expect("revoke");
        assert_eq!(
            crate::peer_policy::authorize(&pool, &one, &two, uid).await.unwrap_err(),
            crate::peer_policy::Refusal::Unrelated,
            "revoking from either side takes the right away",
        );

        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
