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
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use cctui_proto::api::{
    AddUserActionRequest, TickUserActionRequest, USER_ACTION_LIMIT, USER_ACTION_TITLE_MAX,
    UserAction, UserActionKind, UserActionList, UserActionResolver, UserActionResult,
    UserActionStatus,
};
use uuid::Uuid;

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

/// How a tick landed.
pub enum Ticked {
    Applied(UserActionList),
    AlreadyResolved(UserActionList, UserAction),
    Missing,
}

/// Resolve one open item. A tick on an item someone already resolved changes
/// nothing and broadcasts nothing, so a racing user and agent resolve it once.
///
/// The caller gates on [`writable`] first, so a rejected tick can say *why*
/// instead of pretending the item is gone.
pub async fn tick(
    state: &AppState,
    session_id: &str,
    id: Uuid,
    status: UserActionStatus,
    note: Option<&str>,
    by: UserActionResolver,
) -> Result<Ticked, AppError> {
    let done = sqlx::query(
        "UPDATE session_user_actions SET status = $3, note = COALESCE($4::text, note), \
         resolved_at = now(), resolved_by = $5 WHERE id = $1 AND session_id = $2 AND status = \
         'open'",
    )
    .bind(id)
    .bind(session_id)
    .bind(status.as_str())
    .bind(clean_optional(note))
    .bind(by.as_str())
    .execute(&state.pool)
    .await?
    .rows_affected();
    let list = list_of(state, session_id).await?;
    if done > 0 {
        broadcast(state, &list);
        return Ok(Ticked::Applied(list));
    }
    let current = list.items.iter().find(|i| i.id == id).cloned();
    Ok(current.map_or(Ticked::Missing, |item| Ticked::AlreadyResolved(list, item)))
}

fn already_resolved(item: &UserAction) -> String {
    let by = item.resolved_by.map_or_else(String::new, |r| format!(" by the {}", r.as_str()));
    format!("this item was already {}{by}; left unchanged", item.status.as_str())
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
        Err(_) => Ticked::Missing,
    };
    Ok(Json(match ticked {
        Ticked::Applied(list) => UserActionResult { error: None, added: None, list },
        Ticked::AlreadyResolved(list, item) => {
            UserActionResult { error: Some(already_resolved(&item)), added: None, list }
        }
        Ticked::Missing => UserActionResult {
            error: Some(format!("no user action {} on this session", req.id)),
            added: None,
            list: list_of(&state, &session_id).await?,
        },
    }))
}

// ---- user-facing: the route's `Resource(Session, ..)` guard is the gate ----

/// `GET /api/v1/sessions/{id}/user-actions`.
pub async fn get_list(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<UserActionList>, AppError> {
    Ok(Json(list_of(&state, &session_id).await?))
}

#[derive(Debug, serde::Deserialize)]
pub struct UiTick {
    pub status: UserActionStatus,
    #[serde(default)]
    pub note: Option<String>,
}

/// The list after a UI tick. `applied` is false when the item was already
/// resolved, in which case nothing changed.
#[derive(Debug, serde::Serialize)]
pub struct UiTickResult {
    #[serde(flatten)]
    pub list: UserActionList,
    pub applied: bool,
}

/// `POST /api/v1/sessions/{id}/user-actions/{aid}/tick`.
pub async fn ui_tick(
    State(state): State<AppState>,
    Path((session_id, action_id)): Path<(String, Uuid)>,
    Json(req): Json<UiTick>,
) -> Result<Json<UiTickResult>, AppError> {
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
    match ticked {
        Ticked::Applied(list) => Ok(Json(UiTickResult { list, applied: true })),
        Ticked::AlreadyResolved(list, _) => Ok(Json(UiTickResult { list, applied: false })),
        Ticked::Missing => {
            Err(AppError::new(StatusCode::NOT_FOUND, "no such user action on this session"))
        }
    }
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

    #[test]
    fn the_ui_tick_answer_is_the_list_plus_an_applied_flag() {
        let out = UiTickResult {
            list: UserActionList { session_id: "s".to_owned(), items: Vec::new() },
            applied: false,
        };
        let v = serde_json::to_value(&out).unwrap();
        assert_eq!(v, serde_json::json!({ "session_id": "s", "items": [], "applied": false }));
    }

    async fn open_item(pool: &sqlx::PgPool, sid: &str) -> Uuid {
        sqlx::query_scalar(
            "INSERT INTO session_user_actions (session_id, title) VALUES ($1, 'Plug the yubikey') \
             RETURNING id",
        )
        .bind(sid)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn an_agent_tick_after_the_users_leaves_the_users_resolution() {
        let Some(pool) =
            test_pool("an_agent_tick_after_the_users_leaves_the_users_resolution").await
        else {
            return;
        };
        let sid = seed(&pool, "ua-guard").await;
        let id = open_item(&pool, &sid).await;
        let state = AppState::for_test(pool.clone());
        let mut rx = state.bus.subscribe_server();

        let Json(first) = ui_tick(
            State(state.clone()),
            Path((sid.clone(), id)),
            Json(UiTick { status: UserActionStatus::Done, note: Some("plugged".to_owned()) }),
        )
        .await
        .unwrap();
        assert!(first.applied);
        assert!(matches!(
            &*rx.try_recv().unwrap().event,
            cctui_proto::ws::ServerEvent::UserActions { .. }
        ));
        let before = list(&pool, &sid).await.unwrap().remove(0);

        let second = tick(
            &state,
            &sid,
            id,
            UserActionStatus::Dropped,
            Some("gave up"),
            UserActionResolver::Agent,
        )
        .await
        .unwrap();
        let Ticked::AlreadyResolved(_, current) = second else {
            panic!("a second tick must not apply");
        };
        assert_eq!(
            already_resolved(&current),
            "this item was already done by the user; left unchanged"
        );
        assert!(rx.try_recv().is_err(), "a tick that changed nothing must not broadcast");

        let after = list(&pool, &sid).await.unwrap().remove(0);
        assert_eq!(after.resolved_by, Some(UserActionResolver::User));
        assert_eq!(after.status, UserActionStatus::Done);
        assert_eq!(after.note.as_deref(), Some("plugged"));
        assert_eq!(after.resolved_at, before.resolved_at);

        let Json(again) = ui_tick(
            State(state),
            Path((sid, id)),
            Json(UiTick { status: UserActionStatus::Done, note: None }),
        )
        .await
        .unwrap();
        assert!(!again.applied);
        assert_eq!(again.list.items[0].resolved_by, Some(UserActionResolver::User));
    }

    #[tokio::test]
    async fn two_concurrent_ticks_resolve_the_item_once() {
        let Some(pool) = test_pool("two_concurrent_ticks_resolve_the_item_once").await else {
            return;
        };
        let sid = seed(&pool, "ua-race").await;
        let id = open_item(&pool, &sid).await;
        let state = AppState::for_test(pool.clone());
        let (a, b) = tokio::join!(
            tick(&state, &sid, id, UserActionStatus::Done, None, UserActionResolver::User),
            tick(&state, &sid, id, UserActionStatus::Dropped, None, UserActionResolver::Agent),
        );
        let outcomes = [a.unwrap(), b.unwrap()];
        let applied = outcomes.iter().filter(|t| matches!(t, Ticked::Applied(_))).count();
        let refused = outcomes.iter().filter(|t| matches!(t, Ticked::AlreadyResolved(..))).count();
        assert_eq!((applied, refused), (1, 1));
        let item = list(&pool, &sid).await.unwrap().remove(0);
        let winner = if matches!(outcomes[0], Ticked::Applied(_)) {
            (UserActionStatus::Done, UserActionResolver::User)
        } else {
            (UserActionStatus::Dropped, UserActionResolver::Agent)
        };
        assert_eq!((item.status, item.resolved_by), (winner.0, Some(winner.1)));
    }

    #[tokio::test]
    async fn a_tick_on_an_unknown_id_is_missing() {
        let Some(pool) = test_pool("a_tick_on_an_unknown_id_is_missing").await else { return };
        let sid = seed(&pool, "ua-missing").await;
        let state = AppState::for_test(pool);
        let got = tick(
            &state,
            &sid,
            Uuid::new_v4(),
            UserActionStatus::Done,
            None,
            UserActionResolver::Agent,
        )
        .await
        .unwrap();
        assert!(matches!(got, Ticked::Missing));
    }

    #[tokio::test]
    async fn an_admin_reads_another_users_list_under_the_session_read_rule() {
        use crate::auth::{AuthContext, Scope};
        let Some(pool) =
            test_pool("an_admin_reads_another_users_list_under_the_session_read_rule").await
        else {
            return;
        };
        let sid = seed(&pool, "ua-admin").await;
        sqlx::query("INSERT INTO session_user_actions (session_id, title) VALUES ($1, 'x')")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        let caller = |admin| AuthContext {
            user_id: Uuid::new_v4(),
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: if admin { [Scope::Admin].into() } else { std::collections::BTreeSet::new() },
        };
        assert_eq!(
            crate::authz::authorize_session_read(&caller(false), &sid, &pool).await,
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(crate::authz::authorize_session_read(&caller(true), &sid, &pool).await, Ok(()));
        let state = AppState::for_test(pool);
        let Json(got) = get_list(State(state), Path(sid)).await.unwrap();
        assert_eq!(got.items.len(), 1);
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
