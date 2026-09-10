//! Bookmarks — a cross-session collection of saved messages.
//!
//! A row snapshots the message text rather than pointing at `stream_events`:
//! sessions are archived and deleted, so a pure `(session_id, seq)` reference
//! is a bookmark that silently empties. The reference is kept *as well*, and
//! `ON DELETE SET NULL` degrades it to a labelled dead link.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::routes::sessions::ilike_contains;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, TS)]
#[ts(export)]
pub struct Bookmark {
    pub id: Uuid,
    /// Source session, or `None` once that session has been deleted.
    pub session_id: Option<String>,
    /// Position of the message within the source transcript.
    pub seq: Option<i64>,
    pub message_id: Option<String>,
    pub title: String,
    /// The snapshotted message text, as Markdown.
    pub body: String,
    pub role: String,
    /// Denormalised at save time so a dead-link bookmark still says where it
    /// came from.
    pub session_name: Option<String>,
    pub note: Option<String>,
    pub message_ts: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct CreateBookmark {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub seq: Option<i64>,
    #[serde(default)]
    pub message_id: Option<String>,
    pub title: String,
    pub body: String,
    pub role: String,
    #[serde(default)]
    pub session_name: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
    /// Message timestamp (epoch millis on the wire, as the UI carries it).
    pub message_ts: i64,
}

/// `PATCH` payload — title and note only; the snapshot itself is immutable.
#[derive(Debug, Clone, Deserialize, TS)]
#[ts(export)]
pub struct UpdateBookmark {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub note: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    /// Free text over title/body/note.
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<i64>,
    /// Keyset page cursor: return rows strictly older than this `created_at`.
    #[serde(default)]
    pub before: Option<DateTime<Utc>>,
}

const SELECT_COLS: &str = "id, session_id, seq, message_id, title, body, role, session_name, \
     note, message_ts, created_at";

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;
const TITLE_MAX: usize = 120;

/// First non-empty line of `body`, trimmed to ~120 chars — the default title
/// when the client does not supply one.
#[must_use]
pub fn default_title(body: &str) -> String {
    let line = body.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
    if line.chars().count() <= TITLE_MAX {
        return line.to_owned();
    }
    let cut: String = line.chars().take(TITLE_MAX - 1).collect();
    format!("{}…", cut.trim_end())
}

/// Every free-text term must match at least one of title/body/note (AND across
/// terms, OR across columns). Terms come from the shared query tokenizer so
/// quoting behaves the same as session search.
fn q_terms(q: Option<&str>) -> Vec<String> {
    let Some(raw) = q.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let terms = cctui_query::parse(raw).free_text_terms();
    if terms.is_empty() { vec![raw.to_owned()] } else { terms }
}

pub async fn list_bookmarks(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<ListQuery>,
) -> Result<Json<Vec<Bookmark>>, StatusCode> {
    let limit = params.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let terms = q_terms(params.q.as_deref());

    // $1 owner filter, $2 `before` cursor, $3 limit, then one bind per term.
    let mut sql = format!(
        "SELECT {SELECT_COLS} FROM bookmarks \
         WHERE ($1::uuid IS NULL OR user_id = $1) \
           AND ($2::timestamptz IS NULL OR created_at < $2)"
    );
    for i in 0..terms.len() {
        let p = i + 4;
        sql.push_str(&format!(
            " AND (title ILIKE ${p} OR body ILIKE ${p} OR COALESCE(note, '') ILIKE ${p})"
        ));
    }
    sql.push_str(" ORDER BY created_at DESC LIMIT $3");

    let mut query = sqlx::query_as::<_, Bookmark>(sqlx::AssertSqlSafe(sql))
        .bind(ctx.owner_filter())
        .bind(params.before)
        .bind(limit);
    for t in &terms {
        query = query.bind(ilike_contains(t));
    }
    let rows = query.fetch_all(&state.pool).await.map_err(|e| db_err(&e))?;
    Ok(Json(rows))
}

pub async fn create_bookmark(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<CreateBookmark>,
) -> Result<(StatusCode, Json<Bookmark>), StatusCode> {
    if req.body.trim().is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let title = match req.title.trim() {
        "" => default_title(&req.body),
        t => t.to_owned(),
    };
    let message_ts = DateTime::from_timestamp_millis(req.message_ts).unwrap_or_else(Utc::now);
    // A session the caller cannot see must not become a back-link, and a stale
    // id would fail the FK with a 500 — resolve it first and drop it if it is
    // not (or no longer) reachable.
    let session_id = match req.session_id.as_deref() {
        Some(sid) => sqlx::query_scalar::<_, String>(
            "SELECT id FROM sessions WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
        )
        .bind(sid)
        .bind(ctx.owner_filter())
        .fetch_optional(&state.pool)
        .await
        .map_err(|e| db_err(&e))?,
        None => None,
    };

    let row: Bookmark = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "INSERT INTO bookmarks (user_id, session_id, seq, message_id, title, body, role, \
         session_name, note, message_ts) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10) RETURNING {SELECT_COLS}"
    )))
    .bind(ctx.user_id)
    .bind(&session_id)
    .bind(req.seq)
    .bind(&req.message_id)
    .bind(&title)
    .bind(&req.body)
    .bind(&req.role)
    .bind(&req.session_name)
    .bind(req.note.as_deref().map(str::trim).filter(|s| !s.is_empty()))
    .bind(message_ts)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub async fn update_bookmark(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
    Json(req): Json<UpdateBookmark>,
) -> Result<Json<Bookmark>, StatusCode> {
    let title = req.title.as_deref().map(str::trim).filter(|s| !s.is_empty());
    // The editor always round-trips the whole note, so a null/blank one clears it.
    let note = req.note.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let row: Option<Bookmark> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE bookmarks SET title = COALESCE($3, title), note = $4 \
         WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2) RETURNING {SELECT_COLS}"
    )))
    .bind(id)
    .bind(ctx.owner_filter())
    .bind(title)
    .bind(note)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| db_err(&e))?;
    row.map(Json).ok_or(StatusCode::NOT_FOUND)
}

pub async fn delete_bookmark(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, StatusCode> {
    let res =
        sqlx::query("DELETE FROM bookmarks WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)")
            .bind(id)
            .bind(ctx.owner_filter())
            .execute(&state.pool)
            .await
            .map_err(|e| db_err(&e))?;
    if res.rows_affected() == 0 {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn db_err(e: &sqlx::Error) -> StatusCode {
    tracing::error!("db error: {e}");
    StatusCode::INTERNAL_SERVER_ERROR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_title_takes_the_first_non_empty_line() {
        assert_eq!(default_title("\n\n  Wrap-up  \nrest of body"), "Wrap-up");
    }

    #[test]
    fn default_title_of_an_empty_body_is_empty() {
        assert_eq!(default_title("\n   \n"), "");
    }

    #[test]
    fn default_title_is_trimmed_to_120_chars() {
        let t = default_title(&"x".repeat(500));
        assert_eq!(t.chars().count(), TITLE_MAX);
        assert!(t.ends_with('…'));
    }

    #[test]
    fn q_terms_splits_on_whitespace_and_ignores_blank_queries() {
        assert!(q_terms(None).is_empty());
        assert!(q_terms(Some("   ")).is_empty());
        assert_eq!(q_terms(Some("alpha beta")), vec!["alpha", "beta"]);
    }

    #[test]
    fn q_terms_keeps_a_quoted_phrase_whole() {
        assert_eq!(q_terms(Some("\"wrap up\"")), vec!["wrap up"]);
    }

    async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    /// The whole point of the snapshot design: deleting the source session must
    /// leave the bookmark readable, with `session_id` nulled (dead link) rather
    /// than the row cascading away.
    #[tokio::test]
    async fn bookmark_survives_deletion_of_its_source_session() {
        let Some(pool) = test_pool("bookmark_survives_deletion_of_its_source_session").await else {
            return;
        };
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        let sid = format!("cct992-{}", Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("cct992-{uid}"))
            .bind(format!("h992-{uid}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk992-{machine}"))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(&pool)
        .await
        .unwrap();

        let bid: Uuid = sqlx::query_scalar(
            "INSERT INTO bookmarks (user_id, session_id, seq, title, body, role, session_name, \
             message_ts) VALUES ($1, $2, 7, 'Wrap-up', 'the report body', 'assistant', \
             'nightly build', now()) RETURNING id",
        )
        .bind(uid)
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();

        sqlx::query("DELETE FROM sessions WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();

        let row: Option<(Option<String>, String, String, Option<String>)> = sqlx::query_as(
            "SELECT session_id, title, body, session_name FROM bookmarks WHERE id = $1",
        )
        .bind(bid)
        .fetch_optional(&pool)
        .await
        .unwrap();
        let (session_id, title, body, session_name) =
            row.expect("bookmark must outlive its source session");
        assert!(session_id.is_none(), "session_id nulled → dead link, not a deleted bookmark");
        assert_eq!(title, "Wrap-up");
        assert_eq!(body, "the report body", "the snapshotted body is still readable");
        assert_eq!(
            session_name.as_deref(),
            Some("nightly build"),
            "denormalised name still says where it came from"
        );

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.unwrap();
    }
}
