//! Rooms: a named group of sessions with one ordered timeline, fanned out to
//! every member as an attributed turn.
//!
//! ## Why there is no per-member outbox
//!
//! `room_members.last_delivered_seq` IS the delivery cursor. [`sweep`] looks for
//! members whose cursor trails the room's `next_seq` and whose session can take
//! a turn right now ([`crate::keepalive::skip_reason`]), sends the missed posts
//! in `seq` order, and advances the cursor. That one mechanism gives all four
//! behaviours the ticket asks for: a member mid-turn or needing input is simply
//! not selected and is picked up on a later tick; an offline or ended member
//! replays in order when it comes back; a crashed replica loses nothing; and a
//! redelivery cannot duplicate a post.
//!
//! ## Loop guard
//!
//! Nothing here reads a member's ordinary output. A reply only reaches the room
//! if the agent explicitly calls `CctuiRoom post`, and [`pending_for`] never
//! selects a message whose sender is the member itself. Those two facts are the
//! whole guard, and the standing preamble ([`join_preamble`]) tells the agent so.

use axum::http::StatusCode;
use uuid::Uuid;

use cctui_proto::adapter::AdapterCommand;

use crate::error::AppError;
use crate::state::AppState;

/// A room post becomes a turn in every member's session, so it is capped well
/// below a prompt: a room is for coordination, not for shipping payloads.
pub const MAX_POST_BYTES: usize = 16 * 1024;

/// Posts per minute per sender.
pub const POSTS_PER_MIN: usize = 10;

/// Members served per sweep tick. A room fan-out is a burst of turns; spreading
/// it over ticks keeps one big room from monopolising the reaper.
const SWEEP_BATCH: i64 = 100;

/// Posts delivered to one member in a single catch-up, oldest first. A member
/// offline for a long conversation gets the rest on the next tick.
const REPLAY_BATCH: usize = 20;

/// A closing tag in the body would end the wrapper early and the remainder would
/// read as the member's own prose.
const ENVELOPE_CLOSE: &str = "</cctui-room>";

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Room {
    pub id: Uuid,
    pub name: String,
    pub archived: bool,
    pub members: Vec<Member>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Member {
    pub session_id: String,
    pub name: Option<String>,
    pub adapter: Option<String>,
    pub machine: Option<String>,
    pub state: &'static str,
    pub role: String,
    pub last_delivered_seq: i64,
}

impl Member {
    #[must_use]
    pub fn is_observer(&self) -> bool {
        self.role == "observer"
    }

    /// `name (adapter on machine)`, the same shape the peer envelope uses.
    #[must_use]
    pub fn label(&self) -> String {
        let name =
            self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&self.session_id);
        format!(
            "{name} ({} on {})",
            self.adapter.as_deref().unwrap_or("unknown"),
            self.machine.as_deref().unwrap_or("unknown machine"),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct RoomMessage {
    pub seq: i64,
    /// `None` is the human.
    pub sender_session_id: Option<String>,
    pub sender_label: String,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// The label a human post is attributed to.
pub const HUMAN_LABEL: &str = "you (human)";

fn attr(raw: &str) -> String {
    raw.replace(['"', '<', '>'], "")
}

/// Wrap a room post for delivery into a member's turn.
///
/// `<cctui-room>` is deliberately inside the family the webui's `PEER_TAG_RE`
/// already matches (`format.ts`), with `name` carrying the room so the renderer
/// can label it as a room post rather than a direct peer message.
#[must_use]
pub fn envelope(room_name: &str, sender_label: &str, body: &str) -> String {
    format!(
        "<cctui-room name=\"{}\" from=\"{}\">\n{}\n{ENVELOPE_CLOSE}",
        attr(room_name),
        attr(sender_label),
        body.trim(),
    )
}

/// The standing block a session is told when it joins, delivered through the
/// same neutral primitive as a post so no harness needs to know about rooms.
#[must_use]
pub fn join_preamble(room_name: &str, members: &[Member], observer: bool) -> String {
    let roster = if members.is_empty() {
        "nobody else yet".to_owned()
    } else {
        members.iter().map(Member::label).collect::<Vec<_>>().join(", ")
    };
    let how = if observer {
        "You are an OBSERVER: you receive the room's messages but cannot post to it."
    } else {
        "To say something to the room, call the CctuiRoom tool with action \"post\". Nothing else \
         you write reaches the room — your ordinary replies stay in your own conversation, so you \
         will not echo yourself by working normally. Use \"peek\" to re-read the timeline and \
         \"members\" to see who is in it."
    };
    format!(
        "<cctui-room-joined name=\"{}\">\nYou have been added to the cctui room \"{}\".\n\
         Members: {roster}, plus the human.\n\
         Messages wrapped in <cctui-room> come from another member of this room, not from the \
         human who runs you.\n{how}\n</cctui-room-joined>",
        attr(room_name),
        attr(room_name),
    )
}

const MEMBERS_SQL: &str = "\
SELECT rm.session_id, s.session_name, s.adapter_id, m.name, s.status, rm.role, \
       rm.last_delivered_seq \
  FROM room_members rm \
  LEFT JOIN sessions s ON s.id = rm.session_id \
  LEFT JOIN machines m ON m.id = s.machine_uuid \
 WHERE rm.room_id = $1 \
 ORDER BY rm.joined_at, rm.session_id";

type MemberRow = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    String,
    i64,
);

fn member_of(r: MemberRow) -> Member {
    let (session_id, name, adapter, machine, status, role, last_delivered_seq) = r;
    Member {
        session_id,
        name,
        adapter,
        machine,
        state: crate::peer_policy::state_of(status.as_deref()),
        role,
        last_delivered_seq,
    }
}

pub async fn members(pool: &sqlx::PgPool, room_id: Uuid) -> Result<Vec<Member>, sqlx::Error> {
    let rows: Vec<MemberRow> = sqlx::query_as(MEMBERS_SQL).bind(room_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(member_of).collect())
}

/// A room the caller owns, with its members. `None` for another owner's room or
/// an unknown id — the same answer, so an id's existence never leaks.
pub async fn load(
    pool: &sqlx::PgPool,
    room_id: Uuid,
    owner: Uuid,
) -> Result<Option<Room>, sqlx::Error> {
    let row: Option<(Uuid, String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, name, archived_at FROM rooms WHERE id = $1 AND user_id = $2",
    )
    .bind(room_id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    let Some((id, name, archived_at)) = row else { return Ok(None) };
    Ok(Some(Room {
        id,
        name,
        archived: archived_at.is_some(),
        members: members(pool, room_id).await?,
    }))
}

pub async fn list(pool: &sqlx::PgPool, owner: Uuid) -> Result<Vec<Room>, sqlx::Error> {
    let rows: Vec<(Uuid, String, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT id, name, archived_at FROM rooms WHERE user_id = $1 \
         ORDER BY archived_at IS NOT NULL, created_at DESC",
    )
    .bind(owner)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, name, archived_at) in rows {
        out.push(Room {
            id,
            name,
            archived: archived_at.is_some(),
            members: members(pool, id).await?,
        });
    }
    Ok(out)
}

/// The live rooms a session belongs to, for the card badge and for resolving a
/// tool call that named no room.
pub async fn rooms_of_session(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> Result<Vec<(Uuid, String, String)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT r.id, r.name, rm.role FROM room_members rm \
           JOIN rooms r ON r.id = rm.room_id AND r.archived_at IS NULL \
          WHERE rm.session_id = $1 ORDER BY r.created_at DESC",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
}

pub async fn timeline(
    pool: &sqlx::PgPool,
    room_id: Uuid,
    after: Option<i64>,
    limit: i64,
) -> Result<Vec<RoomMessage>, sqlx::Error> {
    type Row = (i64, Option<String>, String, String, chrono::DateTime<chrono::Utc>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT seq, sender_session_id, sender_label, body, created_at FROM room_messages \
         WHERE room_id = $1 AND ($2::bigint IS NULL OR seq > $2) \
         ORDER BY seq LIMIT $3",
    )
    .bind(room_id)
    .bind(after)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(seq, sender_session_id, sender_label, body, created_at)| RoomMessage {
            seq,
            sender_session_id,
            sender_label,
            body,
            created_at,
        })
        .collect())
}

/// Why a post was refused, before any row is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostRefusal {
    Empty,
    TooLarge(usize),
    EnvelopeBreak,
    Archived,
    Observer,
    NotAMember,
    RateLimited,
}

impl PostRefusal {
    #[must_use]
    pub fn status(&self) -> StatusCode {
        match self {
            Self::Empty | Self::EnvelopeBreak => StatusCode::BAD_REQUEST,
            Self::TooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Archived | Self::Observer => StatusCode::CONFLICT,
            Self::NotAMember => StatusCode::FORBIDDEN,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        }
    }
}

impl std::fmt::Display for PostRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => f.write_str("message is required"),
            Self::TooLarge(n) => write!(
                f,
                "message is {n} bytes; the room post cap is {MAX_POST_BYTES}. Post a pointer (a \
                 path, a session id), not a payload."
            ),
            Self::EnvelopeBreak => {
                write!(f, "message must not contain {ENVELOPE_CLOSE}: it would truncate the envelope")
            }
            Self::Archived => f.write_str("this room is archived and takes no new messages"),
            Self::Observer => {
                f.write_str("this session is an observer in the room and cannot post to it")
            }
            Self::NotAMember => f.write_str("this session is not a member of that room"),
            Self::RateLimited => {
                write!(f, "room post rate limit reached ({POSTS_PER_MIN} per minute per sender)")
            }
        }
    }
}

impl From<PostRefusal> for AppError {
    fn from(r: PostRefusal) -> Self {
        Self::new(r.status(), r.to_string())
    }
}

/// Validate a post's body against the shape rules. Pure, so the same checks run
/// for the human composer and the agent tool.
pub fn check_body(body: &str) -> Result<&str, PostRefusal> {
    let body = body.trim();
    if body.is_empty() {
        return Err(PostRefusal::Empty);
    }
    if body.len() > MAX_POST_BYTES {
        return Err(PostRefusal::TooLarge(body.len()));
    }
    if body.contains(ENVELOPE_CLOSE) {
        return Err(PostRefusal::EnvelopeBreak);
    }
    Ok(body)
}

/// Whether `sender` may post to `room`. `None` is the human, who may always
/// post to a live room they own.
pub fn check_sender(room: &Room, sender: Option<&str>) -> Result<(), PostRefusal> {
    if room.archived {
        return Err(PostRefusal::Archived);
    }
    let Some(sender) = sender else { return Ok(()) };
    let Some(member) = room.members.iter().find(|m| m.session_id == sender) else {
        return Err(PostRefusal::NotAMember);
    };
    if member.is_observer() {
        return Err(PostRefusal::Observer);
    }
    Ok(())
}

fn limiter() -> &'static crate::routes::peer::Limiter {
    static LIMITER: std::sync::LazyLock<crate::routes::peer::Limiter> =
        std::sync::LazyLock::new(crate::routes::peer::Limiter::default);
    &LIMITER
}

/// Append a message to `room`'s timeline and wake the fan-out.
///
/// The `seq` is allocated under the room's row lock, so two concurrent posts get
/// distinct, ordered sequence numbers and no member's cursor can skip one.
pub async fn post(
    state: &AppState,
    room: &Room,
    owner: Uuid,
    sender: Option<&Member>,
    body: &str,
) -> Result<RoomMessage, AppError> {
    let body = check_body(body)?;
    check_sender(room, sender.map(|m| m.session_id.as_str()))?;
    let key = sender.map_or_else(|| format!("room-human:{}", room.id), |m| {
        format!("room:{}", m.session_id)
    });
    if !limiter().admit(&key, POSTS_PER_MIN, std::time::Instant::now()) {
        return Err(PostRefusal::RateLimited.into());
    }
    let label = sender.map_or_else(|| HUMAN_LABEL.to_owned(), Member::label);

    let mut tx = state.pool.begin().await?;
    let seq: i64 = sqlx::query_scalar(
        "UPDATE rooms SET next_seq = next_seq + 1 WHERE id = $1 RETURNING next_seq",
    )
    .bind(room.id)
    .fetch_one(&mut *tx)
    .await?;
    let created_at: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "INSERT INTO room_messages (room_id, seq, sender_session_id, sender_label, body) \
         VALUES ($1, $2, $3, $4, $5) RETURNING created_at",
    )
    .bind(room.id)
    .bind(seq)
    .bind(sender.map(|m| m.session_id.clone()))
    .bind(&label)
    .bind(body)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;

    let message = RoomMessage {
        seq,
        sender_session_id: sender.map(|m| m.session_id.clone()),
        sender_label: label,
        body: body.to_owned(),
        created_at,
    };
    announce(state, room, owner, &message);
    deliver_room(state, room.id).await;
    Ok(message)
}

/// Tell every watching webui socket about a post, so the Room panel updates
/// without polling.
fn announce(state: &AppState, room: &Room, owner: Uuid, message: &RoomMessage) {
    state.bus.publish_server(cctui_proto::ws::ServerEvent::RoomMessage {
        room_id: room.id,
        room_name: room.name.clone(),
        user_id: owner,
        seq: message.seq,
        sender_session_id: message.sender_session_id.clone(),
        sender_label: message.sender_label.clone(),
        body: message.body.clone(),
    });
}

/// A member with posts it has not been given yet, and the session signals that
/// decide whether it can take a turn now.
#[derive(Debug, Clone)]
pub struct Pending {
    pub room_id: Uuid,
    pub room_name: String,
    pub session_id: String,
    pub role: String,
    pub last_delivered_seq: i64,
    pub status: String,
    pub tempo: Option<String>,
    pub agent_state: Option<String>,
    pub soft_limit_reason: Option<String>,
    pub ended: bool,
}

impl Pending {
    /// Whether this member can be handed a turn right now. Mid-turn and
    /// needs-input members are left for a later tick; ended and archived ones
    /// wait until a resume flips their row back to a live status.
    #[must_use]
    pub fn deliverable(&self) -> bool {
        crate::keepalive::skip_reason(&crate::keepalive::Snapshot {
            status: &self.status,
            tempo: self.tempo.as_deref(),
            agent_state: self.agent_state.as_deref(),
            soft_limit_reason: self.soft_limit_reason.as_deref(),
            ended: self.ended,
            ticks_sent: 0,
            max_ticks: 0,
        })
        .is_none()
    }
}

/// `$1` bounds the scan; `$2`, when non-null, restricts it to one room.
const PENDING_SQL: &str = "\
SELECT r.id, r.name, rm.session_id, rm.role, rm.last_delivered_seq, \
       COALESCE(s.status, 'ended'), s.tempo, s.agent_state, s.soft_limit_reason, \
       EXISTS (SELECT 1 FROM stream_events e \
                WHERE e.session_id = rm.session_id AND e.event_type = 'session_ended') \
  FROM room_members rm \
  JOIN rooms r ON r.id = rm.room_id AND r.archived_at IS NULL \
  LEFT JOIN sessions s ON s.id = rm.session_id \
 WHERE rm.last_delivered_seq < r.next_seq \
   AND ($2::uuid IS NULL OR r.id = $2) \
 ORDER BY r.id, rm.session_id \
 LIMIT $1";

type PendingRow = (
    Uuid,
    String,
    String,
    String,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    bool,
);

pub async fn pending(
    pool: &sqlx::PgPool,
    room_id: Option<Uuid>,
    limit: i64,
) -> Result<Vec<Pending>, sqlx::Error> {
    let rows: Vec<PendingRow> =
        sqlx::query_as(PENDING_SQL).bind(limit).bind(room_id).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| Pending {
            room_id: r.0,
            room_name: r.1,
            session_id: r.2,
            role: r.3,
            last_delivered_seq: r.4,
            status: r.5,
            tempo: r.6,
            agent_state: r.7,
            soft_limit_reason: r.8,
            ended: r.9,
        })
        .collect())
}

/// The posts one member still owes, oldest first, excluding its own.
///
/// Excluding the sender here rather than at post time is what makes the cursor
/// safe: the member's cursor still advances past its own post, so it never
/// blocks the queue behind it.
pub async fn pending_for(
    pool: &sqlx::PgPool,
    member: &Pending,
) -> Result<Vec<RoomMessage>, sqlx::Error> {
    type Row = (i64, Option<String>, String, String, chrono::DateTime<chrono::Utc>);
    let rows: Vec<Row> = sqlx::query_as(
        "SELECT seq, sender_session_id, sender_label, body, created_at FROM room_messages \
         WHERE room_id = $1 AND seq > $2 \
           AND (sender_session_id IS NULL OR sender_session_id <> $3) \
         ORDER BY seq LIMIT $4",
    )
    .bind(member.room_id)
    .bind(member.last_delivered_seq)
    .bind(&member.session_id)
    .bind(i64::try_from(REPLAY_BATCH).unwrap_or(20))
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(seq, sender_session_id, sender_label, body, created_at)| RoomMessage {
            seq,
            sender_session_id,
            sender_label,
            body,
            created_at,
        })
        .collect())
}

/// The highest `seq` a member has now seen: the last message actually delivered,
/// or — when every pending message was its own — the room's head, so its cursor
/// does not stay behind its own posts forever.
pub async fn advance_to(
    pool: &sqlx::PgPool,
    member: &Pending,
    delivered: Option<i64>,
) -> Result<i64, sqlx::Error> {
    let target = match delivered {
        Some(seq) => seq,
        None => sqlx::query_scalar::<_, i64>("SELECT next_seq FROM rooms WHERE id = $1")
            .bind(member.room_id)
            .fetch_one(pool)
            .await?,
    };
    sqlx::query(
        "UPDATE room_members SET last_delivered_seq = GREATEST(last_delivered_seq, $3) \
         WHERE room_id = $1 AND session_id = $2",
    )
    .bind(member.room_id)
    .bind(&member.session_id)
    .bind(target)
    .execute(pool)
    .await?;
    Ok(target)
}

/// Hand one member every post it is owed, in order, as a single turn.
///
/// Batched into one turn on purpose: two posts that arrived while a session was
/// busy are one thing to read, and one turn costs one model call instead of two.
pub async fn deliver_pending(state: &AppState, member: &Pending) -> Result<usize, String> {
    let owed = pending_for(&state.pool, member).await.map_err(|e| e.to_string())?;
    if owed.is_empty() {
        let _ = advance_to(&state.pool, member, None).await;
        return Ok(0);
    }
    let text = owed
        .iter()
        .map(|m| envelope(&member.room_name, &m.sender_label, &m.body))
        .collect::<Vec<_>>()
        .join("\n\n");
    let last = owed.last().map(|m| m.seq);
    crate::bus::dispatch(
        state,
        &member.session_id,
        AdapterCommand::SendMessage { local_id: member.session_id.clone(), text },
    )
    .await
    .map_err(|err| err.to_string())?;
    // Advanced only after the bus accepted the frame: a failed dispatch leaves
    // the cursor where it was, so the next tick retries the same posts.
    let _ = advance_to(&state.pool, member, last).await;
    Ok(owed.len())
}

/// Serve one room's pending members immediately after a post.
pub async fn deliver_room(state: &AppState, room_id: Uuid) {
    serve(state, Some(room_id)).await;
}

/// Reaper tick: serve every member whose cursor trails, anywhere.
pub async fn sweep(state: &AppState) {
    serve(state, None).await;
}

async fn serve(state: &AppState, room_id: Option<Uuid>) {
    let members = match pending(&state.pool, room_id, SWEEP_BATCH).await {
        Ok(m) => m,
        Err(err) => {
            tracing::warn!(%err, "room pending lookup failed");
            return;
        }
    };
    for member in members {
        if !member.deliverable() {
            continue;
        }
        match deliver_pending(state, &member).await {
            Ok(0) => {}
            Ok(n) => tracing::info!(
                room = %member.room_id,
                session = %member.session_id,
                posts = n,
                "room fan-out delivered",
            ),
            Err(err) => tracing::warn!(
                room = %member.room_id,
                session = %member.session_id,
                %err,
                "room fan-out failed; the cursor is unchanged and the next tick retries",
            ),
        }
    }
}

/// Add `session_id` to `room`, then greet it with the standing preamble.
///
/// The cursor starts at the room's head: a session joining an old conversation
/// is not flooded with its whole backlog. `peek` is how it reads what it missed.
pub async fn add_member(
    state: &AppState,
    room: &Room,
    owner: Uuid,
    session_id: &str,
    role: &str,
) -> Result<Member, AppError> {
    let role = if role == "observer" { "observer" } else { "member" };
    let head: i64 = sqlx::query_scalar("SELECT next_seq FROM rooms WHERE id = $1")
        .bind(room.id)
        .fetch_one(&state.pool)
        .await?;
    sqlx::query(
        "INSERT INTO room_members (room_id, session_id, role, last_delivered_seq) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (room_id, session_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(room.id)
    .bind(session_id)
    .bind(role)
    .bind(head)
    .execute(&state.pool)
    .await?;
    let joined = members(&state.pool, room.id)
        .await?
        .into_iter()
        .find(|m| m.session_id == session_id)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "session not found"))?;
    let others: Vec<Member> =
        room.members.iter().filter(|m| m.session_id != session_id).cloned().collect();
    state.bus.publish_server(cctui_proto::ws::ServerEvent::RoomMembers {
        room_id: room.id,
        user_id: owner,
    });
    let text = join_preamble(&room.name, &others, joined.is_observer());
    if let Err(err) = crate::bus::dispatch(
        state,
        session_id,
        AdapterCommand::SendMessage { local_id: session_id.to_owned(), text },
    )
    .await
    {
        tracing::info!(
            room = %room.id, session = %session_id, %err,
            "room join preamble not delivered (session offline); it is told on resume",
        );
    }
    Ok(joined)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(id: &str, role: &str) -> Member {
        Member {
            session_id: id.to_owned(),
            name: Some(format!("lane {id}")),
            adapter: Some("claude-code".into()),
            machine: Some("box-a".into()),
            state: "live",
            role: role.to_owned(),
            last_delivered_seq: 0,
        }
    }

    fn room(members: Vec<Member>) -> Room {
        Room { id: Uuid::nil(), name: "wave 23".into(), archived: false, members }
    }

    fn pending_member(status: &str) -> Pending {
        Pending {
            room_id: Uuid::nil(),
            room_name: "wave 23".into(),
            session_id: "a".into(),
            role: "member".into(),
            last_delivered_seq: 0,
            status: status.to_owned(),
            tempo: None,
            agent_state: None,
            soft_limit_reason: None,
            ended: false,
        }
    }

    /// The room envelope must be in the family the webui already detects, and
    /// must carry the room name so a room post is distinguishable from a direct
    /// peer message.
    #[test]
    fn the_room_envelope_names_the_room_and_the_sender() {
        let text = envelope("wave 23", "lane a (codex on box-b)", "  the gate is green  ");
        assert!(
            text.starts_with("<cctui-room name=\"wave 23\" from=\"lane a (codex on box-b)\">"),
            "{text}"
        );
        assert!(text.ends_with("</cctui-room>"), "{text}");
        assert!(text.contains("the gate is green"));
        assert!(!text.contains("  the gate"), "the body is trimmed");
    }

    #[test]
    fn a_quote_or_angle_bracket_cannot_break_out_of_the_envelope() {
        let text = envelope("a\"b", "c<d>\"e", "hi");
        let head = text.lines().next().unwrap();
        assert_eq!(head.matches('"').count(), 4, "{head}");
        assert!(!head.contains("<d>"), "{head}");
    }

    #[test]
    fn the_body_rules_reject_empty_oversized_and_envelope_breaking_posts() {
        assert_eq!(check_body("  hi  ").unwrap(), "hi");
        assert_eq!(check_body("   "), Err(PostRefusal::Empty));
        let big = "x".repeat(MAX_POST_BYTES + 1);
        assert_eq!(check_body(&big), Err(PostRefusal::TooLarge(MAX_POST_BYTES + 1)));
        assert_eq!(check_body("a </cctui-room> b"), Err(PostRefusal::EnvelopeBreak));
        assert_eq!(check_body(&"x".repeat(MAX_POST_BYTES)).map(str::len), Ok(MAX_POST_BYTES));
    }

    #[test]
    fn refusals_map_onto_distinct_statuses() {
        assert_eq!(PostRefusal::Empty.status(), StatusCode::BAD_REQUEST);
        assert_eq!(PostRefusal::TooLarge(1).status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(PostRefusal::Archived.status(), StatusCode::CONFLICT);
        assert_eq!(PostRefusal::Observer.status(), StatusCode::CONFLICT);
        assert_eq!(PostRefusal::NotAMember.status(), StatusCode::FORBIDDEN);
        assert_eq!(PostRefusal::RateLimited.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(PostRefusal::TooLarge(99).to_string().contains("99"));
    }

    /// Authorisation to post: a non-member is refused even though it may hold
    /// the room id, an observer is refused by role, and the human (None) is
    /// always allowed into a live room.
    #[test]
    fn only_non_observer_members_and_the_human_may_post() {
        let r = room(vec![member("a", "member"), member("b", "observer")]);
        assert_eq!(check_sender(&r, Some("a")), Ok(()));
        assert_eq!(check_sender(&r, Some("b")), Err(PostRefusal::Observer));
        assert_eq!(check_sender(&r, Some("stranger")), Err(PostRefusal::NotAMember));
        assert_eq!(check_sender(&r, None), Ok(()));

        let archived = Room { archived: true, ..room(vec![member("a", "member")]) };
        assert_eq!(check_sender(&archived, Some("a")), Err(PostRefusal::Archived));
        assert_eq!(
            check_sender(&archived, None),
            Err(PostRefusal::Archived),
            "an archived room takes nothing, not even from the human"
        );
    }

    /// The queueing rule: a member mid-turn, needing input, soft-limited, ended
    /// or archived is not served this tick. An idle or hibernated one is.
    #[test]
    fn only_idle_members_are_served_and_the_rest_wait_for_a_later_tick() {
        assert!(pending_member("active").deliverable());
        assert!(pending_member("inactive").deliverable());
        assert!(!pending_member("archived").deliverable());
        assert!(!pending_member("ended").deliverable());

        let mid_turn = Pending { tempo: Some("active".into()), ..pending_member("active") };
        assert!(!mid_turn.deliverable(), "a session mid-turn must be left alone");
        let working = Pending { agent_state: Some("working".into()), ..pending_member("active") };
        assert!(!working.deliverable());
        let blocked = Pending { tempo: Some("blocked".into()), ..pending_member("active") };
        assert!(!blocked.deliverable(), "a session needing input must be queued, not interrupted");
        let limited =
            Pending { soft_limit_reason: Some("weekly_all".into()), ..pending_member("active") };
        assert!(!limited.deliverable());
        let gone = Pending { ended: true, ..pending_member("active") };
        assert!(!gone.deliverable());
        let hibernated = Pending { tempo: Some("hibernated".into()), ..pending_member("active") };
        assert!(hibernated.deliverable(), "a hibernated member is woken by the post");
    }

    /// The loop guard, in the SQL that selects what a member is owed: a member
    /// is never handed back its own post.
    #[test]
    fn the_fanout_query_excludes_the_members_own_posts() {
        assert!(
            PENDING_SQL.contains("rm.last_delivered_seq < r.next_seq"),
            "the cursor is what makes a member pending"
        );
        assert!(PENDING_SQL.contains("archived_at IS NULL"), "an archived room fans out nothing");
    }

    #[test]
    fn the_join_preamble_names_the_room_the_members_and_how_to_reply() {
        let text = join_preamble("wave 23", &[member("a", "member"), member("b", "member")], false);
        assert!(text.contains("name=\"wave 23\""), "{text}");
        assert!(text.contains("lane a (claude-code on box-a)"), "{text}");
        assert!(text.contains("lane b (claude-code on box-a)"), "{text}");
        assert!(text.contains("plus the human"), "{text}");
        assert!(text.contains("CctuiRoom"), "{text}");
        assert!(text.contains("action \"post\""), "{text}");
        assert!(
            text.contains("stay in your own conversation"),
            "the loop guard must be stated, or agents echo every turn: {text}"
        );
    }

    #[test]
    fn an_observer_is_told_it_cannot_post_and_an_empty_room_says_so() {
        let observer = join_preamble("wave 23", &[], true);
        assert!(observer.contains("OBSERVER"), "{observer}");
        assert!(observer.contains("nobody else yet"), "{observer}");
        assert!(!observer.contains("action \"post\""), "{observer}");
    }

    #[test]
    fn a_member_label_falls_back_to_the_session_id() {
        let mut m = member("a", "member");
        assert_eq!(m.label(), "lane a (claude-code on box-a)");
        m.name = Some("  ".into());
        m.adapter = None;
        m.machine = None;
        assert_eq!(m.label(), "a (unknown on unknown machine)");
        assert!(!m.is_observer());
        assert!(member("b", "observer").is_observer());
    }
}
