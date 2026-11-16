use cctui_proto::api::outcome::{OutcomeStatus, SessionOutcome};
use cctui_proto::api::{UserActionResolver, UserActionStatus};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgExecutor, PgPool};
use uuid::Uuid;

use super::kind::{APPROVAL, USER_ACTION};
use super::store::{self, Edge, NewNotification, NotificationRecord, Routing};
use crate::state::AppState;

const SUBJECT_CLOSED: &str = "subject_closed";
const SUPERSEDED: &str = "superseded";

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

pub async fn on_outcome(state: &AppState, outcome: &SessionOutcome) {
    let routing = Routing::from_config(&state.config);
    if let Err(e) = outcome_declared(&state.pool, routing, outcome).await {
        tracing::warn!(session_id = %outcome.session_id, "approval notification failed: {e}");
    }
}

#[cfg(test)]
#[path = "producers_tests.rs"]
mod tests;
