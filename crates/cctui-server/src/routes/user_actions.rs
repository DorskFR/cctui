//! The session "needs you" list: what an agent is waiting on from the human.
//!
//! Two faces on one table. The daemon posts here for `CctuiUserActionAdd` /
//! `CctuiUserActionTick`; the browser reads the list and ticks items itself. The
//! server is authoritative for both, so the agent's next call shows it the ticks
//! the user made in the UI — nothing is inferred from observed tool calls the way
//! `sessions.todos` is.
//!
//! Every mutation broadcasts the whole list on the session WS, so a second tab
//! updates live without refetching.

use axum::Json;
use axum::extract::{Extension, Path, State};
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::api::{
    AddUserActionRequest, TickUserActionRequest, USER_ACTION_LIMIT, USER_ACTION_TITLE_MAX,
    UserAction, UserActionKind, UserActionList, UserActionResolver, UserActionResult,
    UserActionStatus,
};
use uuid::Uuid;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

type Row = (
    Uuid,
    String,
    Option<String>,
    String,
    bool,
    String,
    Option<String>,
    chrono::DateTime<chrono::Utc>,
    Option<chrono::DateTime<chrono::Utc>>,
    Option<String>,
);

/// Open items first (blocking ahead of the rest), then resolved ones oldest
/// first, so the list a model reads back needs no re-sorting.
const LIST_SQL: &str = "SELECT id, title, detail, kind, blocking, status, note, created_at, \
                        resolved_at, resolved_by FROM session_user_actions WHERE session_id = $1 \
                        ORDER BY (status <> 'open'), (blocking AND status = 'open') DESC, \
                        created_at";

fn to_action(row: Row) -> UserAction {
    let (id, title, detail, kind, blocking, status, note, created_at, resolved_at, resolved_by) =
        row;
    UserAction {
        id,
        title,
        detail,
        kind: UserActionKind::parse(&kind),
        blocking,
        status: UserActionStatus::parse(&status),
        note,
        created_at,
        resolved_at,
        resolved_by: resolved_by.as_deref().and_then(UserActionResolver::parse),
    }
}

/// Trim a model-supplied title to the stored shape. `None` when it is empty:
/// an untitled item is unreadable in the card and worthless to the user.
#[must_use]
pub fn clean_title(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(USER_ACTION_TITLE_MAX).collect())
}

fn clean_optional(raw: Option<&str>) -> Option<String> {
    raw.map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned)
}

pub async fn list(pool: &sqlx::PgPool, session_id: &str) -> Result<Vec<UserAction>, AppError> {
    let rows: Vec<Row> = sqlx::query_as(LIST_SQL).bind(session_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(to_action).collect())
}

async fn list_of(state: &AppState, session_id: &str) -> Result<UserActionList, AppError> {
    Ok(UserActionList {
        session_id: session_id.to_owned(),
        items: list(&state.pool, session_id).await?,
    })
}

/// Publish the current list so every open tab on this session updates.
fn broadcast(state: &AppState, list: &UserActionList) {
    state.bus.publish_server(cctui_proto::ws::ServerEvent::UserActions {
        session_id: list.session_id.clone(),
        actions: list.items.clone(),
    });
}

/// A session that can no longer act on its list still shows it; only writes stop.
async fn writable(pool: &sqlx::PgPool, session_id: &str) -> Result<bool, AppError> {
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
        .bind(session_id)
        .fetch_optional(pool)
        .await?;
    Ok(status.is_some_and(|s| s != "archived"))
}

/// Add one item, or return the existing open item with the same title.
pub async fn add(
    state: &AppState,
    session_id: &str,
    req: &AddUserActionRequest,
) -> Result<UserActionResult, AppError> {
    let mut result =
        UserActionResult { error: None, added: None, list: list_of(state, session_id).await? };
    let Some(title) = clean_title(&req.title) else {
        result.error = Some("title is required and was empty".to_owned());
        return Ok(result);
    };
    if !writable(&state.pool, session_id).await? {
        result.error = Some("this session is archived, so its list is read-only".to_owned());
        return Ok(result);
    }
    if let Some(existing) =
        result.list.items.iter().find(|i| i.status.is_open() && i.title == title)
    {
        result.added = Some(existing.id);
        return Ok(result);
    }
    if result.list.items.len() >= USER_ACTION_LIMIT {
        result.error = Some(format!(
            "this session already holds {USER_ACTION_LIMIT} user actions; tick or drop some before \
             adding more"
        ));
        return Ok(result);
    }
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO session_user_actions (session_id, title, detail, kind, blocking) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
    )
    .bind(session_id)
    .bind(&title)
    .bind(clean_optional(req.detail.as_deref()))
    .bind(req.kind.as_str())
    .bind(req.blocking)
    .fetch_one(&state.pool)
    .await?;
    result.added = Some(id);
    result.list = list_of(state, session_id).await?;
    broadcast(state, &result.list);
    Ok(result)
}

/// Resolve one item. `None` for an unknown id — the caller answers with the list
/// and the reason rather than a failure. Callers gate on [`writable`] first, so a
/// rejected tick can say *why* instead of pretending the item is gone.
pub async fn tick(
    state: &AppState,
    session_id: &str,
    id: Uuid,
    status: UserActionStatus,
    note: Option<&str>,
    by: UserActionResolver,
) -> Result<Option<UserActionList>, AppError> {
    let done = sqlx::query(
        "UPDATE session_user_actions SET status = $3, note = COALESCE($4::text, note), \
         resolved_at = now(), resolved_by = $5 WHERE id = $1 AND session_id = $2",
    )
    .bind(id)
    .bind(session_id)
    .bind(status.as_str())
    .bind(clean_optional(note))
    .bind(by.as_str())
    .execute(&state.pool)
    .await?
    .rows_affected();
    if done == 0 {
        return Ok(None);
    }
    let list = list_of(state, session_id).await?;
    broadcast(state, &list);
    Ok(Some(list))
}

// ---- daemon-facing (machine-key bearer) ----

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

/// `POST /api/v1/daemon/sessions/{id}/user-actions`.
pub async fn daemon_add(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<AddUserActionRequest>,
) -> Result<Json<UserActionResult>, AppError> {
    daemon_session(&state, &headers, &session_id).await?;
    Ok(Json(add(&state, &session_id, &req).await?))
}

/// `POST /api/v1/daemon/sessions/{id}/user-actions/tick`.
pub async fn daemon_tick(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<TickUserActionRequest>,
) -> Result<Json<UserActionResult>, AppError> {
    daemon_session(&state, &headers, &session_id).await?;
    if !writable(&state.pool, &session_id).await? {
        return Ok(Json(UserActionResult {
            error: Some("this session is archived, so its list is read-only".to_owned()),
            added: None,
            list: list_of(&state, &session_id).await?,
        }));
    }
    let ticked = match Uuid::parse_str(req.id.trim()) {
        Ok(id) => {
            tick(
                &state,
                &session_id,
                id,
                req.status,
                req.note.as_deref(),
                UserActionResolver::Agent,
            )
            .await?
        }
        Err(_) => None,
    };
    Ok(Json(match ticked {
        Some(list) => UserActionResult { error: None, added: None, list },
        None => UserActionResult {
            error: Some(format!("no user action {} on this session", req.id)),
            added: None,
            list: list_of(&state, &session_id).await?,
        },
    }))
}

// ---- user-facing (session owner) ----

async fn require_owner(
    state: &AppState,
    ctx: &AuthContext,
    session_id: &str,
) -> Result<(), AppError> {
    match crate::authz::session_owner(session_id, &state.pool).await? {
        Some(owner) if owner == ctx.user_id => Ok(()),
        Some(_) => Err(AppError::new(StatusCode::FORBIDDEN, "not your session")),
        None => Err(AppError::new(StatusCode::NOT_FOUND, "no such session")),
    }
}

/// `GET /api/v1/sessions/{id}/user-actions`.
pub async fn get_list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path(session_id): Path<String>,
) -> Result<Json<UserActionList>, AppError> {
    require_owner(&state, &ctx, &session_id).await?;
    Ok(Json(list_of(&state, &session_id).await?))
}

#[derive(Debug, serde::Deserialize)]
pub struct UiTick {
    pub status: UserActionStatus,
    #[serde(default)]
    pub note: Option<String>,
}

/// `POST /api/v1/sessions/{id}/user-actions/{aid}/tick`.
pub async fn ui_tick(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((session_id, action_id)): Path<(String, Uuid)>,
    Json(req): Json<UiTick>,
) -> Result<Json<UserActionList>, AppError> {
    require_owner(&state, &ctx, &session_id).await?;
    if !writable(&state.pool, &session_id).await? {
        return Err(AppError::new(
            StatusCode::CONFLICT,
            "this session is archived, so its list is read-only",
        ));
    }
    let ticked = tick(
        &state,
        &session_id,
        action_id,
        req.status,
        req.note.as_deref(),
        UserActionResolver::User,
    )
    .await?;
    ticked
        .map(Json)
        .ok_or_else(|| AppError::new(StatusCode::NOT_FOUND, "no such user action on this session"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_title_is_rejected_and_a_long_one_is_trimmed() {
        assert!(clean_title("   ").is_none());
        assert!(clean_title("").is_none());
        assert_eq!(clean_title("  Approve PR #12  ").as_deref(), Some("Approve PR #12"));
        let long = "x".repeat(USER_ACTION_TITLE_MAX + 40);
        assert_eq!(clean_title(&long).unwrap().chars().count(), USER_ACTION_TITLE_MAX);
    }

    #[test]
    fn blank_optionals_are_dropped_rather_than_stored_empty() {
        assert!(clean_optional(Some("  ")).is_none());
        assert!(clean_optional(None).is_none());
        assert_eq!(clean_optional(Some(" token received ")).as_deref(), Some("token received"));
    }

    #[test]
    fn the_list_query_puts_open_blocking_items_first() {
        assert!(LIST_SQL.contains("ORDER BY (status <> 'open')"));
        assert!(LIST_SQL.contains("(blocking AND status = 'open') DESC"));
    }

    async fn test_pool(name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn seed(pool: &sqlx::PgPool, tag: &str) -> String {
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        let sid = format!("{tag}-{}", Uuid::new_v4());
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{tag}-{uid}"))
            .bind(format!("h-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, '/w', 'active')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    #[tokio::test]
    async fn an_item_persists_and_reads_back_open() {
        let Some(pool) = test_pool("an_item_persists_and_reads_back_open").await else { return };
        let sid = seed(&pool, "ua-persist").await;
        sqlx::query(
            "INSERT INTO session_user_actions (session_id, title, kind, blocking) \
             VALUES ($1, 'Approve PR #12', 'decision', true)",
        )
        .bind(&sid)
        .execute(&pool)
        .await
        .unwrap();
        let items = list(&pool, &sid).await.unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Approve PR #12");
        assert_eq!(items[0].kind, UserActionKind::Decision);
        assert!(items[0].blocking);
        assert_eq!(items[0].status, UserActionStatus::Open);
        assert!(items[0].resolved_by.is_none());
    }

    #[tokio::test]
    async fn open_blocking_items_sort_ahead_of_open_and_resolved_ones() {
        let Some(pool) =
            test_pool("open_blocking_items_sort_ahead_of_open_and_resolved_ones").await
        else {
            return;
        };
        let sid = seed(&pool, "ua-order").await;
        for (title, blocking, status) in [
            ("done one", false, "done"),
            ("plain one", false, "open"),
            ("blocking one", true, "open"),
        ] {
            sqlx::query(
                "INSERT INTO session_user_actions (session_id, title, blocking, status) \
                 VALUES ($1, $2, $3, $4)",
            )
            .bind(&sid)
            .bind(title)
            .bind(blocking)
            .bind(status)
            .execute(&pool)
            .await
            .unwrap();
        }
        let titles: Vec<String> =
            list(&pool, &sid).await.unwrap().into_iter().map(|i| i.title).collect();
        assert_eq!(titles, vec!["blocking one", "plain one", "done one"]);
    }

    #[tokio::test]
    async fn a_users_tick_is_recorded_as_resolved_by_user() {
        let Some(pool) = test_pool("a_users_tick_is_recorded_as_resolved_by_user").await else {
            return;
        };
        let sid = seed(&pool, "ua-tick").await;
        let id: Uuid = sqlx::query_scalar(
            "INSERT INTO session_user_actions (session_id, title) VALUES ($1, 'Plug the yubikey') \
             RETURNING id",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE session_user_actions SET status = 'done', resolved_at = now(), \
             resolved_by = 'user' WHERE id = $1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        let item = list(&pool, &sid).await.unwrap().remove(0);
        assert_eq!(item.status, UserActionStatus::Done);
        assert_eq!(item.resolved_by, Some(UserActionResolver::User));
        assert!(item.resolved_at.is_some());
    }

    #[tokio::test]
    async fn deleting_the_session_takes_its_list_with_it() {
        let Some(pool) = test_pool("deleting_the_session_takes_its_list_with_it").await else {
            return;
        };
        let sid = seed(&pool, "ua-cascade").await;
        sqlx::query("INSERT INTO session_user_actions (session_id, title) VALUES ($1, 'x')")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
        assert!(list(&pool, &sid).await.unwrap().is_empty());
    }
}
