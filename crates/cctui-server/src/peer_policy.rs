//! The one authorization policy for session-to-session addressing.
//!
//! Every peer surface — roster, send, history — asks this module the same
//! question: may `caller` address `target`? The answer is always "same owner
//! AND some relation holds", and the relations live in [`PREDICATES`], each a
//! pure function of [`PeerFacts`].
//!
//! ## Adding a relation
//!
//! Topics are a separate feature that widens who may talk to whom without
//! changing anything else here. It adds:
//!
//! 1. a [`Relation`] variant,
//! 2. a field on [`PeerFacts`] carrying the fact its predicate needs (a bool,
//!    or an id the roster can group by),
//! 3. a predicate appended to [`PREDICATES`],
//! 4. the matching clause in [`ROSTER_SQL`]'s `WHERE`/`CASE`.
//!
//! No caller of [`decide`] changes, and the refusal path stays one branch.

use uuid::Uuid;

/// Why the caller may address the target. Ordered most specific first, which is
/// also [`PREDICATES`]' evaluation order, so a pair related two ways reports the
/// tighter relation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// The caller itself. Addressable so `CctuiHistory` can read own history.
    Own,
    Parent,
    Child,
    Sibling,
    /// Both sessions carry the same `sessions.room_id`.
    Room,
    /// A cctuiverse link on another cctui. Never produced by [`decide`]: a
    /// remote peer is resolved through `cctuiverse::link_for_session` before
    /// the same-owner check is ever reached.
    Remote,
}

impl Relation {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Own => "self",
            Self::Parent => "parent",
            Self::Child => "child",
            Self::Sibling => "sibling",
            Self::Room => "room",
            Self::Remote => "remote",
        }
    }
}

/// A refused address, with the reason that is both logged and returned to the
/// model. Never leaks whether a session it may not see exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No such session, or one owned by somebody else: the same answer, so a
    /// probe cannot enumerate another user's session ids.
    Unknown,
    /// Exists, same owner, but no relation holds.
    Unrelated,
}

impl Refusal {
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Unknown => "no such session",
            Self::Unrelated => {
                "not addressable from this session: a peer must be its parent, its child, a \
                 sibling, or in the same room. Call CctuiPeers to see what is reachable, and \
                 ask the human to put you both in a room if you need more."
            }
        }
    }

    /// `403` for a refused relation, `404` for something the caller may not
    /// even know exists.
    #[must_use]
    pub const fn status(self) -> axum::http::StatusCode {
        match self {
            Self::Unknown => axum::http::StatusCode::NOT_FOUND,
            Self::Unrelated => axum::http::StatusCode::FORBIDDEN,
        }
    }
}

/// The session columns the policy and the peer surfaces both need.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionNode {
    pub id: String,
    pub user_id: Option<Uuid>,
    pub parent_id: Option<String>,
    pub machine_uuid: Option<Uuid>,
    pub machine_name: Option<String>,
    pub adapter_id: Option<String>,
    pub name: Option<String>,
    pub status: Option<String>,
    /// The one room this session is in, if any.
    pub room_id: Option<Uuid>,
}

/// `live` / `ended` / `archived`, the three states the roster reports.
/// An idle-but-revivable session is `live`: a message wakes it.
#[must_use]
pub fn state_of(status: Option<&str>) -> &'static str {
    match status {
        Some("archived") => "archived",
        Some("ended" | "failed") => "ended",
        _ => "live",
    }
}

impl SessionNode {
    #[must_use]
    pub fn state(&self) -> &'static str {
        state_of(self.status.as_deref())
    }

    /// Whether a message can still be delivered into this session.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.state() == "live"
    }

    /// `name (adapter on machine)` — the sender label the envelope carries.
    #[must_use]
    pub fn label(&self) -> String {
        let name =
            self.name.as_deref().map(str::trim).filter(|n| !n.is_empty()).unwrap_or(&self.id);
        let adapter = self.adapter_id.as_deref().unwrap_or("unknown");
        let machine = self.machine_name.as_deref().unwrap_or("unknown machine");
        format!("{name} ({adapter} on {machine})")
    }
}

/// Everything [`PREDICATES`] may look at. A new relation adds a field here
/// rather than a database call inside a predicate: the decision stays pure and
/// the table-driven test can enumerate it.
#[derive(Debug, Clone)]
pub struct PeerFacts {
    pub caller: SessionNode,
    pub target: SessionNode,
    /// Both sessions carry the same `sessions.room_id`.
    pub same_room: bool,
}

type Predicate = fn(&PeerFacts) -> Option<Relation>;

/// Evaluated in order; the first match is the reported relation.
const PREDICATES: &[Predicate] = &[tree_relation, same_room];

/// Parent, child, sibling or self within the `parent_id` tree. Two roots are
/// NOT siblings: `parent_id IS NULL` is "no parent", not a shared one.
fn tree_relation(facts: &PeerFacts) -> Option<Relation> {
    let (me, they) = (&facts.caller, &facts.target);
    if me.id == they.id {
        return Some(Relation::Own);
    }
    if me.parent_id.as_deref() == Some(they.id.as_str()) {
        return Some(Relation::Parent);
    }
    if they.parent_id.as_deref() == Some(me.id.as_str()) {
        return Some(Relation::Child);
    }
    match (me.parent_id.as_deref(), they.parent_id.as_deref()) {
        (Some(a), Some(b)) if a == b => Some(Relation::Sibling),
        _ => None,
    }
}

fn same_room(facts: &PeerFacts) -> Option<Relation> {
    facts.same_room.then_some(Relation::Room)
}

/// May `caller` address `target`, and why?
///
/// Ownership is checked once, here: a predicate never sees a cross-owner pair,
/// so no future relation can accidentally bridge two users.
pub fn decide(facts: &PeerFacts) -> Result<Relation, Refusal> {
    let Some(owner) = facts.caller.user_id else { return Err(Refusal::Unknown) };
    if facts.target.user_id != Some(owner) {
        return Err(Refusal::Unknown);
    }
    PREDICATES.iter().find_map(|p| p(facts)).ok_or(Refusal::Unrelated)
}

/// One addressable peer, as the roster reports it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Peer {
    pub session_id: String,
    pub name: Option<String>,
    pub adapter: Option<String>,
    pub machine: Option<String>,
    pub state: &'static str,
    pub relation: String,
}

/// Columns of [`ROSTER_SQL`], in order.
pub type RosterRow =
    (String, Option<String>, Option<String>, Option<String>, Option<String>, String);

/// The roster as one query: the same relations [`PREDICATES`] encodes, in the
/// same precedence, so a session never appears in `CctuiPeers` that `decide`
/// would then refuse.
///
/// `$1` is the calling session id.
pub const ROSTER_SQL: &str = "\
WITH me AS (SELECT id, user_id, parent_id, room_id FROM sessions WHERE id = $1) \
SELECT s.id, s.session_name, s.adapter_id, m.name, s.status, \
       CASE \
         WHEN s.id = me.parent_id THEN 'parent' \
         WHEN s.parent_id = me.id THEN 'child' \
         WHEN me.parent_id IS NOT NULL AND s.parent_id = me.parent_id THEN 'sibling' \
         ELSE 'room' \
       END \
  FROM me \
  JOIN sessions s ON s.user_id = me.user_id AND s.id <> me.id \
  LEFT JOIN machines m ON m.id = s.machine_uuid \
 WHERE s.id = me.parent_id \
    OR s.parent_id = me.id \
    OR (me.parent_id IS NOT NULL AND s.parent_id = me.parent_id) \
    OR (me.room_id IS NOT NULL AND s.room_id = me.room_id) \
 ORDER BY 6, s.id \
 LIMIT 200";

/// `$1` is the calling session id, `$2` the target's.
///
/// The room's own `archived_at` is deliberately NOT a condition. Archiving a room
/// archives its sessions, so the archived state lives on the sessions, where the
/// rest of the policy already reads it: an archived peer stays readable with
/// `CctuiHistory` and refuses a `CctuiSend` (409) exactly like any other archived
/// session, and unarchiving one session restores its reach without having to
/// unarchive the room too.
pub const SAME_ROOM_SQL: &str = "\
SELECT 1 FROM sessions a \
  JOIN sessions b ON b.room_id = a.room_id \
 WHERE a.id = $1 AND b.id = $2 AND a.room_id IS NOT NULL \
 LIMIT 1";

/// `$1` is the session id.
pub const NODE_SQL: &str = "\
SELECT s.id, s.user_id, s.parent_id, s.machine_uuid, m.name, s.adapter_id, s.session_name, \
       s.status, s.room_id \
  FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
 WHERE s.id = $1";

/// Columns of [`NODE_SQL`], in order.
pub type NodeRow = (
    String,
    Option<Uuid>,
    Option<String>,
    Option<Uuid>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<Uuid>,
);

impl From<NodeRow> for SessionNode {
    fn from(r: NodeRow) -> Self {
        let (id, user_id, parent_id, machine_uuid, machine_name, adapter_id, name, status, room_id) =
            r;
        Self {
            id,
            user_id,
            parent_id,
            machine_uuid,
            machine_name,
            adapter_id,
            name,
            status,
            room_id,
        }
    }
}

pub async fn load_node(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> Result<Option<SessionNode>, sqlx::Error> {
    let row: Option<NodeRow> =
        sqlx::query_as(NODE_SQL).bind(session_id).fetch_optional(pool).await?;
    Ok(row.map(SessionNode::from))
}

/// Load both sessions and the share fact, then [`decide`]. A database error
/// refuses rather than grants: the check is fail-closed like the spawn one.
pub async fn authorize(
    pool: &sqlx::PgPool,
    caller_id: &str,
    target_id: &str,
    caller_owner: Uuid,
) -> Result<(Relation, PeerFacts), Refusal> {
    let caller = load_node(pool, caller_id).await.ok().flatten().ok_or(Refusal::Unknown)?;
    if caller.user_id != Some(caller_owner) {
        return Err(Refusal::Unknown);
    }
    let target = load_node(pool, target_id).await.ok().flatten().ok_or(Refusal::Unknown)?;
    let same_room: bool = sqlx::query_scalar::<_, i32>(SAME_ROOM_SQL)
        .bind(caller_id)
        .bind(target_id)
        .fetch_optional(pool)
        .await
        .map_err(|_| Refusal::Unknown)?
        .is_some();
    let facts = PeerFacts { caller, target, same_room };
    decide(&facts).map(|relation| (relation, facts))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, owner: Uuid, parent: Option<&str>) -> SessionNode {
        SessionNode {
            id: id.to_owned(),
            user_id: Some(owner),
            parent_id: parent.map(str::to_owned),
            machine_uuid: None,
            machine_name: Some("box-a".into()),
            adapter_id: Some("claude-code".into()),
            name: Some(format!("s-{id}")),
            status: Some("active".into()),
            room_id: None,
        }
    }

    fn facts(caller: SessionNode, target: SessionNode) -> PeerFacts {
        PeerFacts { caller, target, same_room: false }
    }

    /// The policy table the two tickets specify, in one place.
    #[test]
    fn the_policy_table_holds() {
        let me = Uuid::new_v4();
        let other = Uuid::new_v4();
        let root = node("root", me, None);
        let a = node("a", me, Some("root"));
        let b = node("b", me, Some("root"));
        let grandchild = node("gc", me, Some("a"));
        let unrelated_root = node("loner", me, None);
        let stranger = node("theirs", other, Some("root"));

        let cases: &[(&SessionNode, &SessionNode, Result<Relation, Refusal>)] = &[
            (&a, &a, Ok(Relation::Own)),
            (&a, &root, Ok(Relation::Parent)),
            (&root, &a, Ok(Relation::Child)),
            (&a, &grandchild, Ok(Relation::Child)),
            (&grandchild, &a, Ok(Relation::Parent)),
            (&a, &b, Ok(Relation::Sibling)),
            (&b, &a, Ok(Relation::Sibling)),
            (&a, &unrelated_root, Err(Refusal::Unrelated)),
            (&unrelated_root, &a, Err(Refusal::Unrelated)),
            (&grandchild, &b, Err(Refusal::Unrelated)),
            (&a, &stranger, Err(Refusal::Unknown)),
            (&stranger, &a, Err(Refusal::Unknown)),
        ];
        for (caller, target, want) in cases {
            let got = decide(&facts((*caller).clone(), (*target).clone()));
            assert_eq!(got, *want, "{} → {}", caller.id, target.id);
        }
    }

    /// Two roots share no parent, so they are not siblings — otherwise every
    /// top-level session of the account could address every other.
    #[test]
    fn two_root_sessions_are_not_siblings() {
        let me = Uuid::new_v4();
        assert_eq!(
            decide(&facts(node("r1", me, None), node("r2", me, None))),
            Err(Refusal::Unrelated)
        );
    }

    /// Room membership is the third predicate: it authorises a pair the tree
    /// does not relate, and it outranks an explicit share so the roster and the
    /// decision report the same, more informative, relation.
    #[test]
    fn a_shared_room_authorizes_an_otherwise_unrelated_pair() {
        let me = Uuid::new_v4();
        let mut f = facts(node("r1", me, None), node("r2", me, None));
        assert_eq!(decide(&f), Err(Refusal::Unrelated));
        f.same_room = true;
        assert_eq!(decide(&f), Ok(Relation::Room));
    }

    /// A room never overrides the tree: a child of the caller that also shares a
    /// room with it still reports `child`.
    #[test]
    fn the_tree_relation_still_wins_over_a_shared_room() {
        let me = Uuid::new_v4();
        let mut f = facts(node("root", me, None), node("a", me, Some("root")));
        f.same_room = true;
        assert_eq!(decide(&f), Ok(Relation::Child));
    }

    /// A room can never bridge two owners: ownership is checked before any
    /// predicate runs, so no future relation can accidentally do it either.
    #[test]
    fn a_shared_room_across_owners_is_still_refused() {
        let f = PeerFacts {
            caller: node("mine", Uuid::new_v4(), None),
            target: node("theirs", Uuid::new_v4(), None),
            same_room: true,
        };
        assert_eq!(decide(&f), Err(Refusal::Unknown));
    }

    #[test]
    fn a_session_without_an_owner_can_address_nothing() {
        let me = Uuid::new_v4();
        let mut caller = node("a", me, Some("root"));
        caller.user_id = None;
        assert_eq!(decide(&facts(caller, node("root", me, None))), Err(Refusal::Unknown));
    }

    #[test]
    fn refusals_map_onto_distinct_statuses_and_carry_a_reason() {
        assert_eq!(Refusal::Unrelated.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(Refusal::Unknown.status(), axum::http::StatusCode::NOT_FOUND);
        assert!(Refusal::Unrelated.reason().contains("CctuiPeers"));
    }

    #[test]
    fn session_state_collapses_the_status_vocabulary_onto_three_values() {
        let me = Uuid::new_v4();
        let with = |status: Option<&str>| SessionNode {
            status: status.map(str::to_owned),
            ..node("s", me, None)
        };
        for live in ["new", "active", "inactive", "draft"] {
            assert!(with(Some(live)).is_live(), "{live}");
        }
        assert_eq!(with(Some("archived")).state(), "archived");
        assert_eq!(with(Some("ended")).state(), "ended");
        assert_eq!(with(Some("failed")).state(), "ended");
        assert_eq!(with(None).state(), "live");
    }

    #[test]
    fn the_sender_label_names_the_session_adapter_and_machine() {
        let me = Uuid::new_v4();
        let mut n = node("a", me, None);
        assert_eq!(n.label(), "s-a (claude-code on box-a)");
        n.name = Some("  ".into());
        assert_eq!(n.label(), "a (claude-code on box-a)", "a blank name falls back to the id");
        n.machine_name = None;
        n.adapter_id = None;
        assert_eq!(n.label(), "a (unknown on unknown machine)");
    }

    /// The roster's `CASE` must agree with [`PREDICATES`]: every label it can
    /// emit is a [`Relation`] the policy also knows.
    #[test]
    fn the_roster_sql_labels_match_the_relation_vocabulary() {
        for relation in [Relation::Parent, Relation::Child, Relation::Sibling, Relation::Room] {
            assert!(
                ROSTER_SQL.contains(&format!("'{}'", relation.as_str())),
                "{} missing from the roster CASE",
                relation.as_str()
            );
        }
        assert!(
            !ROSTER_SQL.contains(&format!("'{}'", Relation::Remote.as_str())),
            "remote peers come from cctuiverse links, never from the owner-scoped roster"
        );
        assert!(ROSTER_SQL.contains("s.user_id = me.user_id"), "the roster must stay owner-scoped");
        assert!(
            ROSTER_SQL.contains("s.room_id = me.room_id"),
            "a room is a field on sessions, not a membership table"
        );
        assert!(
            !SAME_ROOM_SQL.contains("archived_at"),
            "the archived state lives on the sessions, not the room: archiving a room \
             archives them, and the rest of the policy already reads a session's state"
        );
    }
}
