use std::time::Duration;

use cctui_proto::api::outcome::{
    DeclareOutcomeRequest, OutcomeSource, OutcomeStatus, parse_text_outcome,
};

use crate::error::AppError;
use crate::state::AppState;

/// The transcript tail can trail the turn-end signal by a poll or two.
const SETTLE: Duration = Duration::from_secs(5);

pub const ENDED_WITHOUT_OUTCOME_HEADLINE: &str = "The turn ended without an outcome";

#[derive(Debug, Clone, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct TurnFacts {
    pub declared: bool,
    pub final_text: Option<String>,
    pub open_user_actions: i64,
    pub live_children: i64,
}

#[must_use]
pub fn decide(facts: &TurnFacts) -> Option<DeclareOutcomeRequest> {
    if facts.declared {
        return None;
    }
    let idle = facts.open_user_actions == 0 && facts.live_children == 0;
    facts.final_text.as_deref().and_then(parse_text_outcome).or_else(|| {
        idle.then(|| DeclareOutcomeRequest {
            status: OutcomeStatus::EndedWithoutOutcome,
            headline: ENDED_WITHOUT_OUTCOME_HEADLINE.to_owned(),
            body: None,
            evidence: Vec::new(),
            source: OutcomeSource::Inferred,
        })
    })
}

/// The turn starts at the newest non-meta user message; an outcome whose
/// `turn_seq` is at or past it belongs to this turn.
pub(super) const FACTS_SQL: &str = "\
    WITH turn AS ( \
        SELECT COALESCE(( \
            SELECT id FROM stream_events \
            WHERE session_id = $1 AND event_type = 'message' AND payload->>'role' = 'user' \
              AND (payload->'meta') IS DISTINCT FROM 'true'::jsonb \
            ORDER BY id DESC LIMIT 1), 0) AS start) \
    SELECT \
        EXISTS (SELECT 1 FROM session_outcomes o \
                WHERE o.session_id = $1 AND COALESCE(o.turn_seq, 0) >= turn.start) AS declared, \
        (SELECT COALESCE( \
                    CASE jsonb_typeof(se.payload->'text') \
                        WHEN 'string' THEN se.payload->>'text' END, \
                    CASE jsonb_typeof(se.payload->'content') \
                        WHEN 'string' THEN se.payload->>'content' END) \
         FROM stream_events se \
         WHERE se.session_id = $1 AND se.event_type = 'message' \
           AND se.payload->>'role' = 'assistant' \
           AND (se.payload->'meta') IS DISTINCT FROM 'true'::jsonb AND se.id > turn.start \
         ORDER BY se.id DESC LIMIT 1) AS final_text, \
        (SELECT count(*) FROM session_user_actions \
         WHERE session_id = $1 AND status = 'open') AS open_user_actions, \
        (SELECT count(*) FROM sessions \
         WHERE parent_id = $1 AND status IN ('active', 'inactive') \
           AND tempo IN ('active', 'blocked')) AS live_children \
    FROM turn";

pub(super) async fn evaluate(state: &AppState, session_id: &str) -> Result<(), AppError> {
    let mut tx = state.pool.begin().await?;
    super::lock_session(&mut tx, session_id).await?;
    let facts: TurnFacts = sqlx::query_as(FACTS_SQL).bind(session_id).fetch_one(&mut *tx).await?;
    let Some(req) = decide(&facts) else { return Ok(()) };
    let req = req
        .normalized()
        .map_err(|e| AppError::new(axum::http::StatusCode::INTERNAL_SERVER_ERROR, e))?;
    let outcome = super::insert(&mut tx, session_id, &req).await?;
    tx.commit().await?;
    super::on_declared(state, &outcome).await;
    Ok(())
}

pub fn turn_ended(state: &AppState, session_id: &str) {
    let (state, session_id) = (state.clone(), session_id.to_owned());
    tokio::spawn(async move {
        tokio::time::sleep(SETTLE).await;
        if let Err(e) = evaluate(&state, &session_id).await {
            tracing::warn!(%session_id, "turn-end outcome not evaluated: {e}");
        }
    });
}
