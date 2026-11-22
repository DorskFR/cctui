//! Read side of the lifecycle event log.
//!
//! Visibility mirrors `GET /sessions`: a non-admin sees an event only when
//! they own its session, its machine, or are its `user_id`; `system.*` kinds
//! are admin-only everywhere.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_proto::api::events::{EventPage, EventRecord};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx::{Postgres, QueryBuilder};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;
use crate::events::Severity;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

/// Query string of the three list routes.
///
/// `kind` is comma-separated and prefix-matched, so `session.` selects the
/// family.
#[derive(Debug, Default, Deserialize)]
pub struct EventQuery {
    #[serde(default)]
    pub before: Option<i64>,
    #[serde(default)]
    pub after: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub machine_id: Option<Uuid>,
    #[serde(default)]
    pub severity: Option<String>,
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub until: Option<DateTime<Utc>>,
}

impl EventQuery {
    fn limit(&self) -> i64 {
        self.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT)
    }

    /// `kind=session.,machine.offline` → the LIKE patterns to match.
    #[must_use]
    pub fn kind_patterns(&self) -> Vec<String> {
        kind_patterns(self.kind.as_deref())
    }
}

#[must_use]
pub fn kind_patterns(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| format!("{}%", k.replace('%', "\\%").replace('_', "\\_")))
        .collect()
}

/// Which rows the caller may read. `None` scopes nothing (admin).
#[derive(Debug, Clone, Copy)]
struct Viewer {
    user: Option<Uuid>,
}

impl Viewer {
    fn of(ctx: &AuthContext) -> Self {
        Self { user: ctx.owner_filter() }
    }
}

/// Shared list body.
///
/// `subject` narrows to one session or machine on the per-subject routes;
/// `q` applies the caller's filters on top.
async fn fetch<S: SessionsContext>(
    state: &S,
    viewer: Viewer,
    subject: Option<(&str, &str)>,
    q: &EventQuery,
) -> Result<EventPage, sqlx::Error> {
    let limit = q.limit();
    let mut qb: QueryBuilder<Postgres> = QueryBuilder::new(
        "SELECT e.id, e.occurred_at, e.kind, e.severity, e.session_id, e.machine_id, e.user_id, \
                e.actor, e.summary, e.detail \
         FROM events e \
         LEFT JOIN sessions s ON s.id = e.session_id \
         LEFT JOIN machines m ON m.id = e.machine_id \
         WHERE TRUE",
    );
    if let Some(user) = viewer.user {
        qb.push(" AND e.kind NOT LIKE 'system.%' AND (e.user_id = ")
            .push_bind(user)
            .push(" OR s.user_id = ")
            .push_bind(user)
            .push(" OR m.user_id = ")
            .push_bind(user)
            .push(")");
    }
    match subject {
        Some(("session", id)) => {
            qb.push(" AND e.session_id = ").push_bind(id.to_owned());
        }
        Some(("machine", id)) => {
            let machine = Uuid::parse_str(id).unwrap_or_default();
            qb.push(" AND e.machine_id = ").push_bind(machine);
        }
        _ => {}
    }
    if let Some(session_id) = &q.session_id {
        qb.push(" AND e.session_id = ").push_bind(session_id.clone());
    }
    if let Some(machine_id) = q.machine_id {
        qb.push(" AND e.machine_id = ").push_bind(machine_id);
    }
    let patterns = q.kind_patterns();
    if !patterns.is_empty() {
        qb.push(" AND e.kind LIKE ANY(").push_bind(patterns).push(")");
    }
    if let Some(raw) = &q.severity {
        let severity = Severity::parse(raw).map_or("none", Severity::as_str);
        qb.push(" AND e.severity = ").push_bind(severity);
    }
    if let Some(since) = q.since {
        qb.push(" AND e.occurred_at >= ").push_bind(since);
    }
    if let Some(until) = q.until {
        qb.push(" AND e.occurred_at <= ").push_bind(until);
    }
    if let Some(before) = q.before {
        qb.push(
            " AND (e.occurred_at, e.id) < (SELECT c.occurred_at, c.id FROM events c WHERE c.id = ",
        )
        .push_bind(before)
        .push(")");
    }
    if let Some(after) = q.after {
        qb.push(
            " AND (e.occurred_at, e.id) > (SELECT c.occurred_at, c.id FROM events c WHERE c.id = ",
        )
        .push_bind(after)
        .push(")");
    }
    qb.push(" ORDER BY e.occurred_at DESC, e.id DESC LIMIT ").push_bind(limit + 1);
    let mut events: Vec<EventRecord> = qb.build_query_as().fetch_all(state.pool()).await?;
    let has_more = events.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    events.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    Ok(EventPage { events, has_more })
}

/// `GET /api/v1/events`
pub async fn list_events<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Query(q): Query<EventQuery>,
) -> Result<Json<EventPage>, AppError> {
    Ok(Json(fetch(&state, Viewer::of(&ctx), None, &q).await?))
}

/// `GET /api/v1/sessions/{id}/events`
pub async fn session_events<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
    Query(q): Query<EventQuery>,
) -> Result<Json<EventPage>, AppError> {
    Ok(Json(fetch(&state, Viewer::of(&ctx), Some(("session", &session_id)), &q).await?))
}

/// `GET /api/v1/machines/{machine_id}/events`
pub async fn machine_events<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path(machine_id): Path<String>,
    Query(q): Query<EventQuery>,
) -> Result<Json<EventPage>, AppError> {
    if Uuid::parse_str(&machine_id).is_err() {
        return Err(AppError::new(StatusCode::NOT_FOUND, "machine not found"));
    }
    Ok(Json(fetch(&state, Viewer::of(&ctx), Some(("machine", &machine_id)), &q).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_patterns_split_trim_and_prefix_match() {
        assert_eq!(
            kind_patterns(Some("session., machine.offline")),
            vec!["session.%", "machine.offline%"]
        );
        assert!(kind_patterns(None).is_empty());
        assert!(kind_patterns(Some(" , ")).is_empty());
        assert_eq!(kind_patterns(Some("a_b")), vec!["a\\_b%"]);
    }

    #[test]
    fn limit_defaults_and_clamps() {
        assert_eq!(EventQuery::default().limit(), DEFAULT_LIMIT);
        assert_eq!(EventQuery { limit: Some(0), ..Default::default() }.limit(), 1);
        assert_eq!(EventQuery { limit: Some(10_000), ..Default::default() }.limit(), MAX_LIMIT);
    }
}
