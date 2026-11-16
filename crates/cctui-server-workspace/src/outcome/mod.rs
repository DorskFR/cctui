#[cfg(test)]
mod tests;
mod turn_end;

use std::collections::HashMap;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::api::outcome::{
    DeclareOutcomeRequest, DeclareOutcomeResult, OutcomeSource, OutcomeStatus, SessionOutcome,
};
use uuid::Uuid;

pub use turn_end::turn_ended;

use crate::error::AppError;
use crate::state::AppState;

#[derive(sqlx::FromRow)]
struct Row {
    id: Uuid,
    session_id: String,
    dispatch_id: Option<Uuid>,
    turn_seq: Option<i64>,
    status: String,
    headline: String,
    body: Option<String>,
    evidence: Vec<String>,
    source: String,
    created_at: chrono::DateTime<chrono::Utc>,
}

macro_rules! columns {
    () => {
        "id, session_id, dispatch_id, turn_seq, status, headline, body, evidence, source, created_at"
    };
}

const INSERT_SQL: &str = concat!(
    "INSERT INTO session_outcomes \
       (session_id, dispatch_id, turn_seq, status, headline, body, evidence, source) \
     VALUES ($1, \
       (SELECT dispatch_id FROM dispatch_identities WHERE session_id = $1 LIMIT 1), \
       (SELECT max(id) FROM stream_events WHERE session_id = $1), \
       $2, $3, $4, $5, $6) \
     RETURNING ",
    columns!()
);

const LATEST_RESULT_FILE_SQL: &str = concat!(
    "SELECT ",
    columns!(),
    " FROM session_outcomes WHERE session_id = $1 AND source = 'result_file' \
     ORDER BY created_at DESC, id DESC LIMIT 1"
);

const LATEST_OF_SQL: &str = concat!(
    "SELECT DISTINCT ON (session_id) ",
    columns!(),
    " FROM session_outcomes WHERE session_id = ANY($1) \
     ORDER BY session_id, created_at DESC, id DESC"
);

fn to_outcome(row: Row) -> SessionOutcome {
    SessionOutcome {
        id: row.id,
        session_id: row.session_id,
        dispatch_id: row.dispatch_id,
        turn_seq: row.turn_seq,
        status: OutcomeStatus::parse(&row.status).unwrap_or(OutcomeStatus::Failed),
        headline: row.headline,
        body: row.body,
        evidence: row.evidence,
        source: OutcomeSource::parse(&row.source).unwrap_or(OutcomeSource::Inferred),
        created_at: row.created_at,
    }
}

/// A declaration and the turn-end inference must never both land for one turn.
async fn lock_session(conn: &mut sqlx::PgConnection, session_id: &str) -> sqlx::Result<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('session_outcome:' || $1))")
        .bind(session_id)
        .execute(conn)
        .await
        .map(|_| ())
}

async fn insert(
    conn: &mut sqlx::PgConnection,
    session_id: &str,
    req: &DeclareOutcomeRequest,
) -> sqlx::Result<SessionOutcome> {
    let row: Row = sqlx::query_as(INSERT_SQL)
        .bind(session_id)
        .bind(req.status.as_str())
        .bind(&req.headline)
        .bind(&req.body)
        .bind(&req.evidence)
        .bind(req.source.as_str())
        .fetch_one(conn)
        .await?;
    Ok(to_outcome(row))
}

async fn latest_result_file(
    conn: &mut sqlx::PgConnection,
    session_id: &str,
) -> sqlx::Result<Option<SessionOutcome>> {
    let row: Option<Row> =
        sqlx::query_as(LATEST_RESULT_FILE_SQL).bind(session_id).fetch_optional(conn).await?;
    Ok(row.map(to_outcome))
}

/// A `RESULT_FILE` relayed twice is stored once. The flag is whether the
/// dispatch callback was queued.
///
/// # Errors
/// A database failure.
pub async fn declare(
    state: &AppState,
    session_id: &str,
    req: &DeclareOutcomeRequest,
) -> Result<(SessionOutcome, bool), AppError> {
    let mut tx = state.pool.begin().await?;
    lock_session(&mut tx, session_id).await?;
    if req.source == OutcomeSource::ResultFile
        && let Some(prev) = latest_result_file(&mut tx, session_id).await?
        && DeclareOutcomeRequest::from(&prev) == *req
    {
        return Ok((prev, false));
    }
    let outcome = insert(&mut tx, session_id, req).await?;
    tx.commit().await?;
    let queued = on_declared(state, &outcome).await;
    Ok((outcome, queued))
}

/// Producers keyed on a stored outcome plug in here.
pub async fn on_declared(state: &AppState, outcome: &SessionOutcome) -> bool {
    state.bus.publish_server(cctui_proto::ws::ServerEvent::SessionOutcome {
        session_id: outcome.session_id.clone(),
        outcome: outcome.clone(),
    });
    crate::notify::producers::on_outcome(state, outcome).await;
    if !outcome.source.is_declared() {
        return false;
    }
    let req = DeclareOutcomeRequest::from(outcome);
    match cctui_server_core::webhook::queue_outcome(&state.pool, &outcome.session_id, &req).await {
        Ok(queued) => queued,
        Err(e) => {
            tracing::warn!(session_id = %outcome.session_id, "outcome callback not queued: {e}");
            false
        }
    }
}

/// Inferred outcomes count: a turn that ended without one supersedes an older
/// result.
///
/// # Errors
/// A database failure.
pub async fn latest_of(
    pool: &sqlx::PgPool,
    session_ids: &[String],
) -> Result<HashMap<String, SessionOutcome>, AppError> {
    if session_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let rows: Vec<Row> = sqlx::query_as(LATEST_OF_SQL).bind(session_ids).fetch_all(pool).await?;
    Ok(rows.into_iter().map(to_outcome).map(|o| (o.session_id.clone(), o)).collect())
}

/// # Errors
/// A database failure.
pub async fn latest(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> Result<Option<SessionOutcome>, AppError> {
    Ok(latest_of(pool, &[session_id.to_owned()]).await?.remove(session_id))
}

async fn daemon_session(
    state: &AppState,
    headers: &HeaderMap,
    session_id: &str,
) -> Result<(), AppError> {
    let caller = crate::routes::spawn_child::machine_user(state, headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let owner: Option<Option<Uuid>> =
        sqlx::query_scalar("SELECT user_id FROM sessions WHERE id = $1")
            .bind(session_id)
            .fetch_optional(&state.pool)
            .await?;
    match owner {
        None => Err(AppError::new(StatusCode::NOT_FOUND, "calling session not found")),
        Some(uid) if uid == Some(caller) => Ok(()),
        Some(_) => Err(AppError::new(StatusCode::FORBIDDEN, "session belongs to another user")),
    }
}

const fn refused(error: String) -> DeclareOutcomeResult {
    DeclareOutcomeResult { error: Some(error), outcome: None, callback_queued: false }
}

/// `POST /api/v1/daemon/sessions/{id}/outcome`.
///
/// # Errors
/// The caller does not own the session, or a database failure.
pub async fn daemon_declare(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<DeclareOutcomeRequest>,
) -> Result<Json<DeclareOutcomeResult>, AppError> {
    daemon_session(&state, &headers, &session_id).await?;
    if !matches!(req.source, OutcomeSource::Tool | OutcomeSource::ResultFile) {
        return Ok(Json(refused("source must be \"tool\" or \"result_file\"".to_owned())));
    }
    let req = match req.normalized() {
        Ok(req) => req,
        Err(e) => return Ok(Json(refused(e))),
    };
    let (outcome, callback_queued) = declare(&state, &session_id, &req).await?;
    Ok(Json(DeclareOutcomeResult { error: None, outcome: Some(outcome), callback_queued }))
}
