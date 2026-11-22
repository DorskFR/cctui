use cctui_proto::api::outcome::{OutcomeStatus, SessionOutcome};
use cctui_proto::api::{UserActionResolver, UserActionStatus};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

use super::kind::{APPROVAL, DISPATCH_FAILED, USER_ACTION};
use super::store::{self, Edge, NewNotification, NotificationRecord, Routing};
use cctui_server_kernel::context::KernelContext;

const SUBJECT_CLOSED: &str = "subject_closed";
const SUPERSEDED: &str = "superseded";

/// The subject a dispatch notice and its failure record share.
pub const DISPATCH_SUBJECT: &str = "dispatch";

pub type Closed = (NotificationRecord, Option<Uuid>);

async fn session_owner<'e>(
    conn: impl PgExecutor<'e>,
    session_id: &str,
) -> sqlx::Result<Option<Uuid>> {
    let owner: Option<Option<Uuid>> = sqlx::query_scalar(
        "SELECT COALESCE(m.user_id, s.user_id) \
         FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid WHERE s.id = $1",
    )
    .bind(session_id)
    .fetch_optional(conn)
    .await?;
    Ok(owner.flatten())
}

fn user_action_buttons() -> Value {
    json!([{ "id": "done", "label": "Done" }, { "id": "dropped", "label": "Drop" }])
}

fn approval_buttons() -> Value {
    json!([
        { "id": "approve", "label": "Approve", "requires_login": true },
        { "id": "reject", "label": "Reject", "requires_login": true },
    ])
}

#[derive(Debug, Clone, Copy)]
pub struct AddedAction<'a> {
    pub session_id: &'a str,
    pub id: Uuid,
    pub title: &'a str,
    pub detail: Option<&'a str>,
    pub blocking: bool,
}

pub async fn user_action_added(
    pool: &PgPool,
    routing: Routing,
    added: &AddedAction<'_>,
) -> sqlx::Result<Option<NotificationRecord>> {
    let Some(user_id) = session_owner(pool, added.session_id).await? else { return Ok(None) };
    let mut n = NewNotification::new(user_id, USER_ACTION, added.title.to_owned());
    n.severity = if added.blocking { "high" } else { "info" };
    n.blocking = added.blocking;
    n.session_id = Some(added.session_id.to_owned());
    n.subject_kind = Some(USER_ACTION.to_owned());
    n.subject_id = Some(added.id.to_string());
    n.collapse_key = Some(format!("{USER_ACTION}:{}", added.id));
    n.body = added.detail.map(str::to_owned);
    n.anchor_seq = latest_seq(pool, added.session_id).await?;
    n.actions = user_action_buttons();
    store::emit(pool, routing, n).await.map(Some)
}

pub async fn user_action_ticked(
    conn: &mut PgConnection,
    action_id: Uuid,
    status: UserActionStatus,
    note: Option<&str>,
    by: UserActionResolver,
) -> sqlx::Result<Option<Closed>> {
    if status.is_open() {
        return Ok(None);
    }
    let open: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM notifications \
         WHERE subject_kind = 'user_action' AND subject_id = $1 \
           AND state IN ('pending', 'delivered', 'read') \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(action_id.to_string())
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = open else { return Ok(None) };
    let edge = match by {
        UserActionResolver::User => {
            Edge::Act { action: status.as_str(), note, channel: "web", by: None }
        }
        UserActionResolver::Agent => Edge::Resolve { reason: SUBJECT_CLOSED },
    };
    store::close_in(conn, id, edge).await
}

async fn approval(
    pool: &PgPool,
    routing: Routing,
    outcome: &SessionOutcome,
) -> sqlx::Result<Option<NotificationRecord>> {
    let Some(user_id) = session_owner(pool, &outcome.session_id).await? else { return Ok(None) };
    let mut n = NewNotification::new(user_id, APPROVAL, outcome.headline.clone());
    n.severity = "high";
    n.blocking = true;
    n.session_id = Some(outcome.session_id.clone());
    n.subject_kind = Some("outcome".to_owned());
    n.subject_id = Some(outcome.id.to_string());
    n.collapse_key = Some(format!("{APPROVAL}:{}", outcome.session_id));
    n.body.clone_from(&outcome.body);
    n.anchor_seq = outcome.turn_seq;
    n.actions = approval_buttons();
    store::emit(pool, routing, n).await.map(Some)
}

async fn supersede_approval(
    conn: &mut PgConnection,
    session_id: &str,
) -> sqlx::Result<Option<NotificationRecord>> {
    let open: Option<Uuid> = sqlx::query_scalar(
        "SELECT id FROM notifications \
         WHERE kind = 'approval' AND session_id = $1 AND collapse_key = $2 \
           AND state IN ('pending', 'delivered', 'read')",
    )
    .bind(session_id)
    .bind(format!("{APPROVAL}:{session_id}"))
    .fetch_optional(&mut *conn)
    .await?;
    let Some(id) = open else { return Ok(None) };
    Ok(store::close_in(conn, id, Edge::Resolve { reason: SUPERSEDED }).await?.map(|(n, _)| n))
}

pub async fn outcome_declared(
    pool: &PgPool,
    routing: Routing,
    outcome: &SessionOutcome,
) -> sqlx::Result<Option<NotificationRecord>> {
    if outcome.status == OutcomeStatus::NeedsInput {
        return approval(pool, routing, outcome).await;
    }
    if !outcome.source.is_declared() {
        return Ok(None);
    }
    let mut tx = pool.begin().await?;
    let closed = supersede_approval(&mut tx, &outcome.session_id).await?;
    tx.commit().await?;
    if let Some(n) = &closed {
        super::in_app::changed(pool, n).await;
    }
    Ok(closed)
}

/// The failure record a dispatch notice stands for.
#[must_use]
pub fn failed_dispatch(n: &NotificationRecord) -> Option<&str> {
    if n.kind != DISPATCH_FAILED || n.subject_kind.as_deref() != Some(DISPATCH_SUBJECT) {
        return None;
    }
    n.subject_id.as_deref()
}

/// Close `user_id`'s open notices about these failed dispatches.
pub async fn dispatch_dismissed(
    pool: &PgPool,
    user_id: Uuid,
    session_ids: &[String],
) -> sqlx::Result<Vec<NotificationRecord>> {
    if session_ids.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pool.begin().await?;
    let open: Vec<Uuid> = sqlx::query_scalar(
        "SELECT id FROM notifications \
         WHERE user_id = $1 AND kind = $2 AND subject_kind = $3 AND subject_id = ANY($4) \
           AND state IN ('pending', 'delivered', 'read') \
         ORDER BY id FOR UPDATE",
    )
    .bind(user_id)
    .bind(DISPATCH_FAILED)
    .bind(DISPATCH_SUBJECT)
    .bind(session_ids)
    .fetch_all(&mut *tx)
    .await?;
    let mut closed = Vec::with_capacity(open.len());
    for id in open {
        let edge = Edge::Resolve { reason: SUBJECT_CLOSED };
        if let Some((n, _)) = store::close_in(&mut tx, id, edge).await? {
            closed.push(n);
        }
    }
    tx.commit().await?;
    if !closed.is_empty() {
        super::in_app::changed_many(pool, user_id, &closed).await;
    }
    Ok(closed)
}

/// The `stream_events` id a notice raised now anchors to.
pub async fn latest_seq<'e>(
    conn: impl PgExecutor<'e>,
    session_id: &str,
) -> sqlx::Result<Option<i64>> {
    sqlx::query_scalar("SELECT max(id) FROM stream_events WHERE session_id = $1")
        .bind(session_id)
        .fetch_one(conn)
        .await
}

pub async fn on_outcome(state: &impl KernelContext, outcome: &SessionOutcome) {
    let routing = Routing::from_config(state.config());
    if let Err(e) = outcome_declared(state.pool(), routing, outcome).await {
        tracing::warn!(session_id = %outcome.session_id, "approval notification failed: {e}");
    }
}

#[cfg(test)]
#[path = "producers_tests.rs"]
mod tests;
