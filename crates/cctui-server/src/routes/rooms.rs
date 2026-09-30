//! Room routes: the small REST surface the webui's room picker needs, and the
//! daemon-authenticated `CctuiRoom` endpoint.
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

/// Session ids in `room`, live ones first, for the archive cascade.
///
/// Read before the room is flagged, because the flag is not what selects them —
/// `room_id` is, and it deliberately survives the archive so the list keeps
/// grouping archived sessions under their room.
async fn sessions_in_room(
    pool: &sqlx::PgPool,
    room_id: Uuid,
) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT id FROM sessions WHERE room_id = $1 AND status <> 'archived' \
         ORDER BY registered_at",
    )
    .bind(room_id)
    .fetch_all(pool)
    .await
}

/// What archiving a room did, per session.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct CascadeCounts {
    pub archived: usize,
    /// Pinned sessions are never swept along, exactly as in a batch archive: a
    /// star means "do not lose this", and a room-wide gesture is precisely where
    /// one would slip through.
    pub skipped_pinned: usize,
}

/// Archive every session in `room`, through the same [`archive_one`] the manual
/// and batch archives use — so the daemon `Remove` is dispatched, descendants go
/// with their parent, classifier signals are cleared and gateway tokens are
/// revoked, without any of that being reimplemented here.
///
/// [`archive_one`]: crate::routes::sessions::archive_one
async fn archive_room_sessions(state: &AppState, room_id: Uuid) -> CascadeCounts {
    let ids = match sessions_in_room(&state.pool, room_id).await {
        Ok(ids) => ids,
        Err(err) => {
            tracing::error!(room = %room_id, %err, "room archive: session lookup failed");
            return CascadeCounts::default();
        }
    };
    let mut counts = CascadeCounts::default();
    for id in &ids {
        match crate::routes::sessions::archive_one(
            state,
            id,
            false,
            cctui_proto::adapter::RemoveInitiator::User,
        )
        .await
        {
            Ok(crate::routes::sessions::ArchiveOutcome::Archived) => counts.archived += 1,
            Ok(crate::routes::sessions::ArchiveOutcome::SkippedPinned) => {
                counts.skipped_pinned += 1;
            }
            Err(err) => tracing::error!(session = %id, %err, "room archive: session db error"),
        }
    }
    tracing::info!(
        room = %room_id,
        archived = counts.archived,
        skipped_pinned = counts.skipped_pinned,
        requested = ids.len(),
        "room archived with its sessions",
    );
    counts
}

/// `PATCH /api/v1/rooms/{id}` — rename, archive or unarchive.
///
/// Archiving a room archives every session in it. Unarchiving does NOT bring them
/// back: an archive is per-session state, sessions are unarchived individually as
/// they always were, and a blanket revive would resurrect jobs the human archived
/// on purpose before the room ever existed.
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
    let mut counts = CascadeCounts::default();
    if let Some(archived) = req.archived {
        let at = archived.then(chrono::Utc::now);
        sqlx::query("UPDATE rooms SET archived_at = $2 WHERE id = $1")
            .bind(id)
            .bind(at)
            .execute(&state.pool)
            .await?;
        if archived {
            counts = archive_room_sessions(&state, id).await;
        }
    }
    state
        .bus
        .publish_server(cctui_proto::ws::ServerEvent::RoomMembers { room_id: id, user_id: ctx.user_id });
    let room = owned(&state, id, ctx.user_id).await?;
    let mut out = json!(room);
    out["archived_sessions"] = json!(counts.archived);
    out["skipped_pinned"] = json!(counts.skipped_pinned);
    Ok(Json(out))
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
            let cast = rooms::post(&state, &room, me.as_ref(), body).await?;
            Ok(Json(json!({
                "room": room.name,
                "room_id": room.id,
                "seq": cast.message.seq,
                "delivered": cast.delivered(),
                "receipts": cast.receipts,
            })))
        }
        "peek" => {
            let messages = rooms::timeline(&state.pool, room.id, None, TIMELINE_PAGE).await?;
            Ok(Json(json!({
                "room": room.name,
                "room_id": room.id,
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

    /// DB-gated: the room lifecycle — sessions across two machines, the reach of
    /// a broadcast (everyone but the sender), the timeline `peek` reads, the
    /// policy relation a room confers, one room per session, and the release on
    /// delete.
    #[tokio::test]
    async fn a_room_relates_its_sessions_and_a_broadcast_reaches_all_but_the_sender() {
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

        // Who a broadcast would reach: every session in the room except the
        // sender. There is no cursor and no queue to inspect — the loop in `post`
        // walks `room.members`, so that list IS the reach.
        let live = rooms::load(&pool, room_id, uid).await.unwrap().unwrap();
        let reach: Vec<&str> = live
            .members
            .iter()
            .filter(|mem| mem.session_id != a)
            .map(|mem| mem.session_id.as_str())
            .collect();
        assert_eq!(reach, vec![b.as_str()], "the sender is excluded, everyone else is in");
        assert!(
            !live.members.iter().any(|mem| mem.session_id == outsider),
            "a session in no room is never reached"
        );
        assert!(live.members.iter().all(|mem| mem.state == "live"));

        // A third post, this one from the human (no sender session), reaches both.
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
        assert_eq!(seq, 3);

        // An archived or ended member stays in the room and is reported as
        // skipped rather than dropped from it.
        sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = $1")
            .bind(&b)
            .execute(&pool)
            .await
            .unwrap();
        let with_archived = rooms::load(&pool, room_id, uid).await.unwrap().unwrap();
        assert_eq!(with_archived.members.len(), 2, "an archived session keeps its room");
        let state_of_b = with_archived
            .members
            .iter()
            .find(|mem| mem.session_id == b)
            .map(|mem| mem.state)
            .unwrap();
        assert_eq!(state_of_b, "archived", "the loop reads this and reports `archived`");
        sqlx::query("UPDATE sessions SET status = 'active' WHERE id = $1")
            .bind(&b)
            .execute(&pool)
            .await
            .unwrap();

        // What `CctuiRoom peek` reads, and its `after=` cursor.
        let all = rooms::timeline(&pool, room_id, None, 200).await.unwrap();
        assert_eq!(all.len(), 3);
        let delta = rooms::timeline(&pool, room_id, Some(2), 200).await.unwrap();
        assert_eq!(delta.len(), 1);
        assert_eq!(delta[0].seq, 3);

        // An archived room refuses new posts.
        sqlx::query("UPDATE rooms SET archived_at = now() WHERE id = $1")
            .bind(room_id)
            .execute(&pool)
            .await
            .unwrap();
        let archived = rooms::load(&pool, room_id, uid).await.unwrap().unwrap();
        assert!(archived.archived);
        assert_eq!(rooms::check_sender(&archived, Some(&a)), Err(PostRefusal::Archived));
        assert_eq!(
            crate::peer_policy::authorize(&pool, &a, &b, uid).await.unwrap().0,
            crate::peer_policy::Relation::Room,
            "an archived room still RELATES its sessions: archiving a room archives them, \
             so the archived state is read off the sessions, and unarchiving one restores \
             its reach without having to unarchive the room",
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

    /// DB-gated: the archive cascade picks exactly the room's own unarchived
    /// sessions — not another room's, not a roomless one, not one already
    /// archived — and `room_id` survives so the grouping does.
    ///
    /// Covers [`sessions_in_room`], the selection the cascade loops over.
    /// `archive_room_sessions` itself is not called: `archive_one` needs an
    /// `AppState` (registry, bus, pending commands) that the DB-gated tests in
    /// this crate do not build.
    #[tokio::test]
    async fn archiving_a_room_selects_exactly_its_own_live_sessions() {
        let Some(url) = crate::routes::gateway::test_db_url("rooms_archive_cascade") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'room-arch', $2)")
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
        let mine: Uuid = sqlx::query_scalar(
            "INSERT INTO rooms (user_id, name) VALUES ($1, 'mine') RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
        let theirs: Uuid = sqlx::query_scalar(
            "INSERT INTO rooms (user_id, name) VALUES ($1, 'theirs') RETURNING id",
        )
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();

        // In `mine`: two live, one ended (still has a row to flag), one already
        // archived. Plus one in the other room and one with no room at all.
        let live_a = Uuid::new_v4().to_string();
        let live_b = Uuid::new_v4().to_string();
        let ended = Uuid::new_v4().to_string();
        let already = Uuid::new_v4().to_string();
        let other_room = Uuid::new_v4().to_string();
        let roomless = Uuid::new_v4().to_string();
        for (id, room, status) in [
            (&live_a, Some(mine), "active"),
            (&live_b, Some(mine), "inactive"),
            (&ended, Some(mine), "ended"),
            (&already, Some(mine), "archived"),
            (&other_room, Some(theirs), "active"),
            (&roomless, None, "active"),
        ] {
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, \
                 adapter_id, status, room_id) \
                 VALUES ($1, $2, '/w', $3, $4, 'claude-code', $5, $6)",
            )
            .bind(id)
            .bind(machine.to_string())
            .bind(uid)
            .bind(machine)
            .bind(status)
            .bind(room)
            .execute(&pool)
            .await
            .expect("seed session");
        }

        let mut picked = sessions_in_room(&pool, mine).await.expect("selection");
        picked.sort();
        let mut want = vec![live_a.clone(), live_b.clone(), ended.clone()];
        want.sort();
        assert_eq!(picked, want, "only this room's not-yet-archived sessions");
        assert!(!picked.contains(&already), "an already-archived session is not re-archived");
        assert!(!picked.contains(&other_room), "another room's sessions are untouched");
        assert!(!picked.contains(&roomless), "a roomless session is untouched");
        assert_eq!(
            sessions_in_room(&pool, theirs).await.unwrap(),
            vec![other_room.clone()],
            "the other room selects its own",
        );

        // The grouping survives: archiving flags the status and leaves room_id.
        sqlx::query("UPDATE rooms SET archived_at = now() WHERE id = $1")
            .bind(mine)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET status = 'archived' WHERE room_id = $1")
            .bind(mine)
            .execute(&pool)
            .await
            .unwrap();
        let room = rooms::load(&pool, mine, uid).await.unwrap().expect("room");
        assert!(room.archived);
        assert_eq!(room.members.len(), 4, "every session keeps its room after the archive");
        assert!(room.members.iter().all(|mem| mem.state == "archived"));
        assert!(
            sessions_in_room(&pool, mine).await.unwrap().is_empty(),
            "re-archiving the room is a no-op"
        );
        assert_eq!(
            rooms::room_of_session(&pool, &live_a).await.unwrap(),
            None,
            "room_of_session only reports LIVE rooms, so the tool stops resolving to it",
        );
        let still: Option<Uuid> = sqlx::query_scalar("SELECT room_id FROM sessions WHERE id = $1")
            .bind(&live_a)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(still, Some(mine), "room_id survives so the list still groups by it");

        // Authz: another owner cannot even see the room, so cannot archive it.
        assert!(rooms::load(&pool, mine, Uuid::new_v4()).await.unwrap().is_none());

        // An archived room still relates its sessions: the archived state lives on
        // the sessions, and unarchiving one restores its reach without the room.
        assert_eq!(
            crate::peer_policy::authorize(&pool, &live_a, &live_b, uid).await.unwrap().0,
            crate::peer_policy::Relation::Room,
        );
        assert_eq!(
            crate::peer_policy::authorize(&pool, &live_a, &other_room, uid).await.unwrap_err(),
            crate::peer_policy::Refusal::Unrelated,
            "a different room is still not a shared room",
        );

        sqlx::query("DELETE FROM rooms WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
