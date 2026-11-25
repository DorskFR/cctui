//! `POST /api/v1/daemon/sessions/{id}/spawn-child` — the server side of the
//! daemon's `CctuiAgent` tool.
//!
//! A session asks its daemon to spawn a subagent; the daemon relays the request
//! here with its machine key. The server is the only place the decision is made:
//! it reads the calling session's [`SpawnCapability`](cctui_proto::api::SpawnCapability) — set by whoever launched
//! that session, never writable by the session itself — and refuses anything the
//! capability does not name. No capability at all ⇒ deny. A child never runs
//! more permissively than its parent, never deeper than the capability's
//! depth, and the budgets granted across one spawn tree never sum past the
//! root's tree ceiling.
//!
//! An authorized child goes down the ordinary spawn path (pre-minted session id,
//! account-bound gateway env, `AdapterCommand::Spawn` over the daemon WS), with
//! `parent_local_id` set so it registers as a real, nested, meterable session.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;

use cctui_proto::api::{
    ApiError, ArchiveChildRequest, ArchiveChildResponse, SpawnChildRequest, SpawnChildResponse,
};
use uuid::Uuid;

use cctui_server_kernel::dispatch_types::Parent;

use crate::context::SessionsContext;
use crate::error::AppError;
use crate::followup::nests_under_parent;

/// Authenticate the caller as a daemon machine key and return its user id.
pub async fn machine_user<S: SessionsContext>(
    state: &S,
    headers: &axum::http::HeaderMap,
) -> Result<Uuid, (StatusCode, Json<ApiError>)> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or_else(|| {
            (StatusCode::UNAUTHORIZED, Json(ApiError { error: "machine key required".into() }))
        })?;
    let ctx = state.auth_config().validate(token).await.ok_or_else(|| {
        (StatusCode::UNAUTHORIZED, Json(ApiError { error: "invalid machine key".into() }))
    })?;
    if ctx.machine_id.is_none() {
        return Err((
            StatusCode::FORBIDDEN,
            Json(ApiError { error: "machine token required".into() }),
        ));
    }
    Ok(ctx.user_id)
}

/// Same resolution the webui spawn runs, keyed on the child's own launch key,
/// so a profile's context set means the same thing from the tool as from the
/// browser.
///
/// The child's spawner is recorded under the same key.
pub async fn remember_child_context<S: SessionsContext>(
    state: &S,
    parent: &Parent,
    child_key: &str,
    adapter: &str,
    cwd: Option<&str>,
    profile: Option<&str>,
) {
    let scope = child_scope(state.pool(), parent, adapter, cwd).await;
    if state.config().features.contains(cctui_proto::features::Feature::Context) {
        let context = crate::routes::context::resolve_for_child(
            state.pool(),
            parent.user_id,
            profile,
            &scope.coords,
        )
        .await;
        crate::routes::context::remember_intent(state.pool(), child_key, &context, &scope).await;
    } else {
        crate::routes::context::ownership::record(state.pool(), child_key, &scope).await;
    }
    let spawner = state.child_spawner(parent).await;
    state.remember_spawner(child_key, spawner, None).await;
}

/// A child runs on its parent's machine, whoever owns it, and a dispatched
/// parent's child is dispatched work too.
pub async fn child_scope(
    pool: &sqlx::PgPool,
    parent: &Parent,
    adapter: &str,
    cwd: Option<&str>,
) -> crate::routes::context::StoredScope {
    use crate::routes::context::{SpawnScope, StoredScope, complete_scope, machine_owner};
    let machine = parent.machine_uuid.to_string();
    let parent_scope =
        match crate::routes::context::ownership::launch_scope(pool, &parent.session_id).await {
            Some(s) => Some(complete_scope(pool, s).await),
            None => None,
        };
    let dispatched = parent_scope.as_ref().is_some_and(|s| s.dispatched);
    StoredScope {
        user_id: parent.user_id,
        coords: SpawnScope {
            machine_owner: machine_owner(pool, &machine).await,
            machine_id: Some(machine),
            working_dir: cwd.map(str::to_owned),
            label_ids: Vec::new(),
            adapter: Some(adapter.to_owned()),
            dispatch: parent_scope.and_then(|s| s.coords.dispatch).filter(|_| dispatched),
        },
        dispatched,
    }
}

/// Children counting against the parent's spawn quota: every child except those
/// that ended in failure or were archived.
///
/// A child that emitted a terminal
/// `session_ended` whose reason kind is anything but `completed` (crashed,
/// killed, adapter error) has freed its slot, so the parent can respawn a replacement.
/// A completed child that is not archived still counts: the parent can reattach
/// to it by `session_id`.
pub async fn live_child_count(exec: impl sqlx::PgExecutor<'_>, parent_id: &str) -> u32 {
    let n: i64 = sqlx::query_scalar(concat!(
        "SELECT count(*) FROM sessions s \
         WHERE s.parent_id = $1 AND ",
        nests_under_parent!("s"),
        " AND s.status <> 'archived' \
         AND NOT EXISTS ( \
             SELECT 1 FROM stream_events e \
             WHERE e.session_id = s.id AND e.event_type = 'session_ended' \
             AND e.payload->'reason'->>'kind' <> 'completed' \
         )",
    ))
    .bind(parent_id)
    .fetch_one(exec)
    .await
    .unwrap_or(0);
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// `POST /api/v1/daemon/sessions/{id}/spawn-child`.
pub async fn spawn_child<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<SpawnChildRequest>,
) -> Result<Json<SpawnChildResponse>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    state.spawn_child(caller, &session_id, &req).await.map(Json)
}

/// `POST /api/v1/daemon/sessions/{id}/message-child` — a follow-up prompt from
/// parent `{id}` into a child it spawned.
///
/// Fail-closed: the target must be a
/// direct child of the caller on the caller's machine; a session can never
/// message an arbitrary session this way.
pub async fn message_child<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::MessageChildRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    state.message_child(caller, &session_id, &req).await?;
    Ok(Json(serde_json::json!({})))
}

pub use cctui_server_kernel::dispatch_types::ChildCompletion;

/// `POST /api/v1/daemon/sessions/{id}/child-completion` — child `{id}`'s
/// finished turn, posted by its daemon when no `CctuiAgent` call is following
/// it any more.
///
/// Delivered into the parent as a turn from the child.
pub async fn child_completion<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(child_id): Path<String>,
    Json(req): Json<ChildCompletion>,
) -> Result<Json<serde_json::Value>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let parent = deliver_completion(&state, caller, &child_id, &req.text).await?;
    Ok(Json(serde_json::json!({ "delivered_to": parent })))
}

/// The parent's id once `text` is in its turn queue. Both sessions must be
/// `caller`'s, and only a live parent is offered the turn.
pub async fn deliver_completion<S: SessionsContext>(
    state: &S,
    caller: Uuid,
    child_id: &str,
    text: &str,
) -> Result<String, AppError> {
    state.deliver_completion(caller, child_id, text).await
}

pub use cctui_server_kernel::dispatch_types::ChildSlots;

pub async fn child_slots<S: SessionsContext>(state: &S, session_id: &str) -> Option<ChildSlots> {
    let cap = state.capability_for(session_id).await.filter(|c| !c.is_empty())?;
    let used = slots_used(state, session_id).await;
    Some(ChildSlots { used, max: cap.max_children, depth_left: cap.max_depth })
}

/// The same count `reserve_child` gates on.
pub async fn slots_used<S: SessionsContext>(state: &S, session_id: &str) -> u32 {
    let pool = state.pool();
    let pending = state.pending_child_count(session_id).await.unwrap_or_else(|e| {
        tracing::warn!(%session_id, error = %e, "pending child count failed");
        0
    });
    live_child_count(pool, session_id).await.saturating_add(pending)
}

/// `POST /api/v1/daemon/sessions/{id}/archive-child` — `{id}` releases a
/// descendant in its own spawn tree.
///
/// Anything outside the tree is a 404 so ids
/// do not leak, and a pinned target is always refused: no agent overrides a pin.
/// A running target is killed, and its own descendants are archived with it.
pub async fn archive_child<S: SessionsContext>(
    State(state): State<S>,
    headers: axum::http::HeaderMap,
    Path(session_id): Path<String>,
    Json(req): Json<ArchiveChildRequest>,
) -> Result<Json<ArchiveChildResponse>, AppError> {
    let caller = machine_user(&state, &headers)
        .await
        .map_err(|(code, Json(e))| AppError::new(code, e.error))?;
    let parent = state.load_parent(&session_id, caller).await?;
    archive_descendant(&state, caller, &parent.session_id, req.session_id.trim()).await.map(Json)
}

pub async fn archive_descendant<S: SessionsContext>(
    state: &S,
    caller: Uuid,
    session_id: &str,
    target: &str,
) -> Result<ArchiveChildResponse, AppError> {
    state.archive_descendant(caller, session_id, target).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// DB-gated: a crashed, killed or archived child frees its quota slot, while
    /// a completed-successful child and a still-running one both keep counting.
    #[tokio::test]
    async fn failed_children_free_their_quota_slot() {
        let Some(url) =
            cctui_server_kernel::test_support::db_url("failed_children_free_their_quota_slot")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'q-test', $2)")
            .bind(uid)
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(&pool)
            .await
            .expect("seed machine");

        let parent_id = Uuid::new_v4().to_string();
        let running = Uuid::new_v4().to_string();
        let completed = Uuid::new_v4().to_string();
        let crashed = Uuid::new_v4().to_string();
        let killed = Uuid::new_v4().to_string();
        let archived_running = Uuid::new_v4().to_string();
        let archived_completed = Uuid::new_v4().to_string();
        for id in [
            &parent_id,
            &running,
            &completed,
            &crashed,
            &killed,
            &archived_running,
            &archived_completed,
        ] {
            let parent = (*id != parent_id).then(|| parent_id.clone());
            sqlx::query(
                "INSERT INTO sessions (id, parent_id, machine_id, working_dir, user_id, \
                 machine_uuid, adapter_id) VALUES ($1, $2, $3, '/w', $4, $5, 'opencode')",
            )
            .bind(id)
            .bind(parent)
            .bind(machine.to_string())
            .bind(uid)
            .bind(machine)
            .execute(&pool)
            .await
            .expect("seed session");
        }
        let end = |id: &str, reason: serde_json::Value| {
            let id = id.to_owned();
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO stream_events (session_id, event_type, payload) \
                     VALUES ($1, 'session_ended', $2)",
                )
                .bind(id)
                .bind(json!({ "reason": reason }))
                .execute(&pool)
                .await
                .expect("seed session_ended");
            }
        };
        end(&completed, json!({ "kind": "completed" })).await;
        end(&crashed, json!({ "kind": "crashed", "detail": "gateway rejected" })).await;
        end(&killed, json!({ "kind": "killed" })).await;
        end(&archived_completed, json!({ "kind": "completed" })).await;
        sqlx::query("UPDATE sessions SET status = 'archived' WHERE id = ANY($1)")
            .bind(vec![archived_running.clone(), archived_completed.clone()])
            .execute(&pool)
            .await
            .expect("archive two children");

        assert_eq!(
            live_child_count(&pool, &parent_id).await,
            2,
            "running + completed count; crashed, killed and archived are freed"
        );

        sqlx::query("DELETE FROM sessions WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM machines WHERE user_id = $1").bind(uid).execute(&pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    async fn seed_two_owners(pool: &sqlx::PgPool) -> (Uuid, Uuid, Uuid, Uuid) {
        use crate::routes::context::{ContextItemSpec, clean_spec, insert};
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        let (ma, mb) = (Uuid::new_v4(), Uuid::new_v4());
        for (uid, machine) in [(a, ma), (b, mb)] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
                .bind(uid)
                .bind(format!("child-reach-{uid}"))
                .bind(format!("kh-{uid}"))
                .execute(pool)
                .await
                .expect("seed user");
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)",
            )
            .bind(machine)
            .bind(uid)
            .bind(machine.to_string())
            .bind(format!("kh-{machine}"))
            .execute(pool)
            .await
            .expect("seed machine");
        }
        let house = clean_spec(ContextItemSpec {
            kind: "rule".into(),
            name: "house".into(),
            title: "House".into(),
            body: "b".into(),
            ..ContextItemSpec::default()
        })
        .expect("valid");
        insert(pool, a, &house, None, None).await.expect("rule");
        (a, b, ma, mb)
    }

    #[tokio::test]
    async fn a_child_on_another_users_machine_gets_none_of_its_parents_default_rules() {
        use crate::routes::context::{
            DispatchCoords, SpawnScope, StoredScope, ownership, resolve_for_child,
        };
        let Some(url) = cctui_server_kernel::test_support::db_url("child_scope_reach") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (a, b, ma, mb) = seed_two_owners(&pool).await;
        let parent = |machine: Uuid| Parent {
            session_id: format!("child-reach-parent-{}", Uuid::new_v4()),
            machine_uuid: machine,
            working_dir: Some("/w".into()),
            user_id: a,
            permission_mode: None,
        };

        let (home, away) = (parent(ma), parent(mb));
        let at_home = child_scope(&pool, &home, "codex", Some("/w")).await;
        let own = |items: Vec<crate::routes::context::ContextItem>| -> Vec<String> {
            items.into_iter().filter(|i| i.user_id == a).map(|i| i.name).collect()
        };
        let got = own(resolve_for_child(&pool, a, None, &at_home.coords).await);
        assert_eq!(got, ["house"]);
        let at_b = child_scope(&pool, &away, "codex", Some("/w")).await;
        assert_eq!(at_b.coords.machine_owner, Some(b));
        assert!(
            own(resolve_for_child(&pool, a, None, &at_b.coords).await).is_empty(),
            "B's machine"
        );

        let worker = parent(ma);
        let coords = SpawnScope {
            dispatch: Some(DispatchCoords {
                owner: a,
                dispatcher: Some("kube".into()),
                profile: None,
            }),
            ..SpawnScope::default()
        };
        ownership::record(
            &pool,
            &worker.session_id,
            &StoredScope { user_id: a, coords, dispatched: true },
        )
        .await;
        let child = child_scope(&pool, &worker, "codex", None).await;
        assert!(child.dispatched);
        assert_eq!(
            child.coords.dispatch.as_ref().and_then(|d| d.dispatcher.as_deref()),
            Some("kube")
        );
        assert!(
            resolve_for_child(&pool, a, None, &child.coords).await.is_empty(),
            "a dispatched parent's child is dispatched work: no opt-in, no rule"
        );

        sqlx::query("DELETE FROM session_context WHERE spawn_key = $1")
            .bind(&worker.session_id)
            .execute(&pool)
            .await
            .ok();
        for table in ["context_items", "machines"] {
            sqlx::query(sqlx::AssertSqlSafe(format!(
                "DELETE FROM {table} WHERE user_id = ANY($1)"
            )))
            .bind(vec![a, b])
            .execute(&pool)
            .await
            .ok();
        }
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(vec![a, b])
            .execute(&pool)
            .await
            .ok();
    }
}
