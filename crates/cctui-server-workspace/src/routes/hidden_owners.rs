//! An admin's hidden users and machines: a view preference, not access
//! control. Their sessions stay reachable by id.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use dashmap::DashMap;
use uuid::Uuid;

use cctui_proto::api::hidden_owners::{HiddenOwner, HiddenOwnerKind, HiddenOwners};
use cctui_proto::ws::ServerEvent;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HiddenSet {
    pub users: HashSet<Uuid>,
    pub machines: HashSet<Uuid>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct SessionPlacement {
    pub owner: Option<Uuid>,
    pub machine: Option<Uuid>,
}

impl HiddenSet {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.users.is_empty() && self.machines.is_empty()
    }

    #[must_use]
    pub fn hides(&self, placement: SessionPlacement) -> bool {
        placement.owner.is_some_and(|u| self.users.contains(&u))
            || placement.machine.is_some_and(|m| self.machines.contains(&m))
    }
}

const CACHE_TTL: Duration = Duration::from_mins(1);

static CACHE: LazyLock<DashMap<Uuid, (Arc<HiddenSet>, Instant)>> = LazyLock::new(DashMap::new);
/// Bumped on every change, so a read racing a write never caches the old set.
static GENERATION: AtomicU64 = AtomicU64::new(0);

pub async fn hidden_set(pool: &sqlx::PgPool, viewer: Uuid) -> Result<Arc<HiddenSet>, sqlx::Error> {
    let cached = CACHE
        .get(&viewer)
        .filter(|entry| entry.1.elapsed() < CACHE_TTL)
        .map(|entry| Arc::clone(&entry.0));
    if let Some(set) = cached {
        return Ok(set);
    }
    let generation = GENERATION.load(Ordering::Acquire);
    let rows: Vec<(String, Uuid)> =
        sqlx::query_as("SELECT kind, target_id FROM user_hidden_owners WHERE viewer_id = $1")
            .bind(viewer)
            .fetch_all(pool)
            .await?;
    let mut set = HiddenSet::default();
    for (kind, id) in rows {
        if kind == HiddenOwnerKind::User.as_str() {
            set.users.insert(id);
        } else {
            set.machines.insert(id);
        }
    }
    let set = Arc::new(set);
    if GENERATION.load(Ordering::Acquire) == generation {
        CACHE.insert(viewer, (Arc::clone(&set), Instant::now()));
    }
    Ok(set)
}

pub fn invalidate(viewer: Uuid) {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    CACHE.remove(&viewer);
}

/// The hidden set that filters `ctx`'s listings: `None` for a non-admin, whose
/// lists are already their own, and for an admin who hides nothing.
pub async fn applying_to(
    pool: &sqlx::PgPool,
    ctx: &AuthContext,
) -> Result<Option<Arc<HiddenSet>>, sqlx::Error> {
    if !ctx.is_admin() {
        return Ok(None);
    }
    let set = hidden_set(pool, ctx.user_id).await?;
    Ok((!set.is_empty()).then_some(set))
}

pub async fn hidden_session_ids(
    pool: &sqlx::PgPool,
    set: &HiddenSet,
    ids: &[String],
) -> Result<HashSet<String>, sqlx::Error> {
    if ids.is_empty() || set.is_empty() {
        return Ok(HashSet::new());
    }
    let users: Vec<Uuid> = set.users.iter().copied().collect();
    let machines: Vec<Uuid> = set.machines.iter().copied().collect();
    let hidden: Vec<String> = sqlx::query_scalar(
        "SELECT s.id FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.id = ANY($1) \
           AND (s.machine_uuid = ANY($2) OR COALESCE(m.user_id, s.user_id) = ANY($3))",
    )
    .bind(ids)
    .bind(&machines)
    .bind(&users)
    .fetch_all(pool)
    .await?;
    Ok(hidden.into_iter().collect())
}

pub async fn placement(
    pool: &sqlx::PgPool,
    session_id: &str,
) -> Result<Option<SessionPlacement>, sqlx::Error> {
    let row: Option<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT COALESCE(m.user_id, s.user_id), s.machine_uuid \
         FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid WHERE s.id = $1",
    )
    .bind(session_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(owner, machine)| SessionPlacement { owner, machine }))
}

fn viewer(ctx: &AuthContext) -> Result<Uuid, AppError> {
    if ctx.is_admin() && ctx.machine_id.is_none() {
        Ok(ctx.user_id)
    } else {
        Err(AppError::new(StatusCode::FORBIDDEN, "hiding owners is an admin view preference"))
    }
}

type OwnerRow = (String, Uuid, Option<String>);

pub async fn list(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<HiddenOwners>, AppError> {
    let viewer = viewer(&ctx)?;
    let rows: Vec<OwnerRow> = sqlx::query_as(
        "SELECT h.kind, h.target_id, \
                CASE WHEN h.kind = 'user' THEN u.name \
                     ELSE COALESCE(m.display_name, m.name) END AS label \
         FROM user_hidden_owners h \
         LEFT JOIN users u ON h.kind = 'user' AND u.id = h.target_id \
         LEFT JOIN machines m ON h.kind = 'machine' AND m.id = h.target_id \
         WHERE h.viewer_id = $1 \
         ORDER BY h.kind DESC, label NULLS LAST, h.target_id",
    )
    .bind(viewer)
    .fetch_all(&state.pool)
    .await?;
    let owners = rows
        .into_iter()
        .filter_map(|(kind, id, label)| {
            Some(HiddenOwner {
                kind: HiddenOwnerKind::parse(&kind)?,
                target_id: id.to_string(),
                label,
            })
        })
        .collect();
    Ok(Json(HiddenOwners { owners }))
}

async fn target_exists(
    pool: &sqlx::PgPool,
    kind: HiddenOwnerKind,
    target: Uuid,
) -> Result<bool, sqlx::Error> {
    let sql = match kind {
        HiddenOwnerKind::User => "SELECT EXISTS (SELECT 1 FROM users WHERE id = $1)",
        HiddenOwnerKind::Machine => {
            "SELECT EXISTS (SELECT 1 FROM machines WHERE id = $1 AND deleted_at IS NULL)"
        }
    };
    sqlx::query_scalar(sql).bind(target).fetch_one(pool).await
}

fn changed(state: &AppState, viewer: Uuid) {
    invalidate(viewer);
    state.bus.publish_server(ServerEvent::HiddenOwnersChanged { user_id: viewer });
}

pub async fn hide(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((kind, target)): Path<(HiddenOwnerKind, Uuid)>,
) -> Result<StatusCode, AppError> {
    let viewer = viewer(&ctx)?;
    if kind == HiddenOwnerKind::User && target == viewer {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "you cannot hide your own sessions"));
    }
    if !target_exists(&state.pool, kind, target).await? {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such user or machine"));
    }
    let inserted = sqlx::query(
        "INSERT INTO user_hidden_owners (viewer_id, kind, target_id) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(viewer)
    .bind(kind.as_str())
    .bind(target)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if inserted > 0 {
        changed(&state, viewer);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unhide(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Path((kind, target)): Path<(HiddenOwnerKind, Uuid)>,
) -> Result<StatusCode, AppError> {
    let viewer = viewer(&ctx)?;
    let removed = sqlx::query(
        "DELETE FROM user_hidden_owners WHERE viewer_id = $1 AND kind = $2 AND target_id = $3",
    )
    .bind(viewer)
    .bind(kind.as_str())
    .bind(target)
    .execute(&state.pool)
    .await?
    .rows_affected();
    if removed > 0 {
        changed(&state, viewer);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, HashMap};

    use axum::extract::{Path, Query, State};
    use axum::http::{HeaderMap, StatusCode};
    use axum::{Extension, Json};
    use uuid::Uuid;

    use cctui_proto::api::hidden_owners::HiddenOwnerKind;
    use cctui_proto::ws::ServerEvent;

    use super::{HiddenSet, SessionPlacement, hide, list, unhide};
    use crate::auth::{AuthContext, Scope};
    use crate::routes::sessions::{ListParams, SearchParams, list_sessions, search_sessions};
    use crate::state::AppState;

    fn ctx(user_id: Uuid, admin: bool) -> AuthContext {
        let mut scopes = BTreeSet::from([Scope::Read]);
        if admin {
            scopes.insert(Scope::Admin);
        }
        AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes }
    }

    async fn test_state(test_name: &str) -> Option<AppState> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        Some(AppState::for_test(pool))
    }

    async fn user(pool: &sqlx::PgPool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(format!("u-{id}"))
            .bind(format!("k-{id}"))
            .execute(pool)
            .await
            .unwrap();
        id
    }

    async fn machine(pool: &sqlx::PgPool, owner: Uuid) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(id)
            .bind(owner)
            .bind(format!("m-{id}"))
            .bind(format!("mk-{id}"))
            .execute(pool)
            .await
            .unwrap();
        id
    }

    async fn session(pool: &sqlx::PgPool, owner: Uuid, machine: Uuid, dir: &str) -> String {
        let id = format!("hidden-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
             VALUES ($1, $2, $2, $3, $4, 'active')",
        )
        .bind(&id)
        .bind(machine)
        .bind(owner)
        .bind(dir)
        .execute(pool)
        .await
        .unwrap();
        id
    }

    struct Fixture {
        state: AppState,
        admin: Uuid,
        other_admin: Uuid,
        user: Uuid,
        user_session: String,
        machine: Uuid,
        machine_session: String,
        token: String,
    }

    async fn fixture(test_name: &str) -> Option<Fixture> {
        let state = test_state(test_name).await?;
        let pool = state.pool.clone();
        let token = format!("hid{}", Uuid::new_v4().simple());
        let (admin, other_admin, owner, hidden_user) =
            (user(&pool).await, user(&pool).await, user(&pool).await, user(&pool).await);
        let user_machine = machine(&pool, hidden_user).await;
        let machine_id = machine(&pool, owner).await;
        let user_session =
            session(&pool, hidden_user, user_machine, &format!("/w/{token}/u")).await;
        let machine_session = session(&pool, owner, machine_id, &format!("/w/{token}/m")).await;
        Some(Fixture {
            state,
            admin,
            other_admin,
            user: hidden_user,
            user_session,
            machine: machine_id,
            machine_session,
            token,
        })
    }

    async fn listed(state: &AppState, viewer: Uuid, show_hidden: bool) -> HashMap<String, bool> {
        let response = list_sessions(
            State(state.clone()),
            Extension(ctx(viewer, true)),
            None,
            Query(ListParams { include_archived: false, show_hidden }),
            HeaderMap::new(),
        )
        .await
        .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        flags(&serde_json::from_slice(&body).unwrap())
    }

    async fn searched(f: &Fixture, viewer: Uuid, show_hidden: bool) -> HashMap<String, bool> {
        let Json(found) = search_sessions(
            State(f.state.clone()),
            Extension(ctx(viewer, true)),
            Query(SearchParams {
                q: f.token.clone(),
                include_archived: true,
                limit: Some(50),
                offset: None,
                cursor: None,
                show_hidden,
            }),
        )
        .await
        .unwrap();
        flags(&serde_json::to_value(found).unwrap())
    }

    fn flags(response: &serde_json::Value) -> HashMap<String, bool> {
        response["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| (s["id"].as_str().unwrap().to_owned(), s["hidden"].as_bool().unwrap_or(false)))
            .collect()
    }

    async fn set_hidden(f: &Fixture, kind: HiddenOwnerKind, target: Uuid, hidden: bool) {
        let path = Path((kind, target));
        let status = if hidden {
            hide(State(f.state.clone()), Extension(ctx(f.admin, true)), path).await.unwrap()
        } else {
            unhide(State(f.state.clone()), Extension(ctx(f.admin, true)), path).await.unwrap()
        };
        assert_eq!(status, StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn hidden_user_sessions_leave_the_admins_list_and_search_only() {
        let Some(f) = fixture("hidden_user_sessions_leave_the_admins_list_and_search_only").await
        else {
            return;
        };
        set_hidden(&f, HiddenOwnerKind::User, f.user, true).await;
        set_hidden(&f, HiddenOwnerKind::User, f.user, true).await;

        let mine = listed(&f.state, f.admin, false).await;
        assert!(!mine.contains_key(&f.user_session), "hidden user's session listed");
        assert_eq!(mine.get(&f.machine_session), Some(&false));
        let found = searched(&f, f.admin, false).await;
        assert!(!found.contains_key(&f.user_session), "hidden user's session found");
        assert_eq!(found.get(&f.machine_session), Some(&false));

        let theirs = listed(&f.state, f.other_admin, false).await;
        assert_eq!(theirs.get(&f.user_session), Some(&false), "another admin still sees it");
        assert!(searched(&f, f.other_admin, false).await.contains_key(&f.user_session));

        assert_eq!(listed(&f.state, f.admin, true).await.get(&f.user_session), Some(&true));
        assert_eq!(searched(&f, f.admin, true).await.get(&f.user_session), Some(&true));

        set_hidden(&f, HiddenOwnerKind::User, f.user, false).await;
        set_hidden(&f, HiddenOwnerKind::User, f.user, false).await;
        assert_eq!(listed(&f.state, f.admin, false).await.get(&f.user_session), Some(&false));
        assert!(searched(&f, f.admin, false).await.contains_key(&f.user_session));
    }

    #[tokio::test]
    async fn hidden_machine_sessions_leave_the_admins_list_and_search_only() {
        let Some(f) = fixture("hidden_machine_sessions_leave_the_admins_list_and_search_only").await
        else {
            return;
        };
        set_hidden(&f, HiddenOwnerKind::Machine, f.machine, true).await;

        let mine = listed(&f.state, f.admin, false).await;
        assert!(!mine.contains_key(&f.machine_session), "hidden machine's session listed");
        assert_eq!(mine.get(&f.user_session), Some(&false));
        assert!(!searched(&f, f.admin, false).await.contains_key(&f.machine_session));
        assert!(listed(&f.state, f.other_admin, false).await.contains_key(&f.machine_session));
        assert_eq!(listed(&f.state, f.admin, true).await.get(&f.machine_session), Some(&true));
        assert_eq!(searched(&f, f.admin, true).await.get(&f.machine_session), Some(&true));

        set_hidden(&f, HiddenOwnerKind::Machine, f.machine, false).await;
        assert!(listed(&f.state, f.admin, false).await.contains_key(&f.machine_session));
    }

    #[tokio::test]
    async fn hidden_owners_lists_labels_and_announces_changes() {
        let Some(f) = fixture("hidden_owners_lists_labels_and_announces_changes").await else {
            return;
        };
        let mut events = f.state.bus.subscribe_server();
        set_hidden(&f, HiddenOwnerKind::User, f.user, true).await;
        set_hidden(&f, HiddenOwnerKind::Machine, f.machine, true).await;

        let Json(got) = list(State(f.state.clone()), Extension(ctx(f.admin, true))).await.unwrap();
        let got: Vec<(HiddenOwnerKind, String, Option<String>)> =
            got.owners.into_iter().map(|o| (o.kind, o.target_id, o.label)).collect();
        assert_eq!(
            got,
            [
                (HiddenOwnerKind::User, f.user.to_string(), Some(format!("u-{}", f.user))),
                (HiddenOwnerKind::Machine, f.machine.to_string(), Some(format!("m-{}", f.machine))),
            ]
        );
        let Json(other) =
            list(State(f.state.clone()), Extension(ctx(f.other_admin, true))).await.unwrap();
        assert!(other.owners.is_empty(), "the list is per admin");

        let mut announced = 0;
        while let Ok(frame) = events.try_recv() {
            if let ServerEvent::HiddenOwnersChanged { user_id } = &*frame.event {
                assert_eq!(*user_id, f.admin);
                announced += 1;
            }
        }
        assert_eq!(announced, 2);
    }

    #[tokio::test]
    async fn hidden_owners_refuses_non_admins_self_and_unknown_targets() {
        let Some(f) = fixture("hidden_owners_refuses_non_admins_self_and_unknown_targets").await
        else {
            return;
        };
        let state = || State(f.state.clone());
        let plain = || Extension(ctx(f.user, false));
        let err = list(state(), plain()).await.unwrap_err();
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
        let path = || Path((HiddenOwnerKind::Machine, f.machine));
        assert_eq!(
            hide(state(), plain(), path()).await.unwrap_err().status(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            unhide(state(), plain(), path()).await.unwrap_err().status(),
            StatusCode::FORBIDDEN
        );
        let mut machine_key = ctx(f.admin, true);
        machine_key.machine_id = Some(f.machine);
        assert_eq!(
            list(state(), Extension(machine_key)).await.unwrap_err().status(),
            StatusCode::FORBIDDEN
        );

        let admin = || Extension(ctx(f.admin, true));
        let own = Path((HiddenOwnerKind::User, f.admin));
        assert_eq!(
            hide(state(), admin(), own).await.unwrap_err().status(),
            StatusCode::BAD_REQUEST
        );
        let unknown = Path((HiddenOwnerKind::User, Uuid::new_v4()));
        assert_eq!(
            hide(state(), admin(), unknown).await.unwrap_err().status(),
            StatusCode::NOT_FOUND
        );
        let own_machine = machine(&f.state.pool, f.admin).await;
        let own_machine = Path((HiddenOwnerKind::Machine, own_machine));
        assert_eq!(hide(state(), admin(), own_machine).await.unwrap(), StatusCode::NO_CONTENT);
    }

    #[test]
    fn hidden_set_hides_by_owner_or_machine() {
        let (user, machine) = (Uuid::new_v4(), Uuid::new_v4());
        let set = HiddenSet {
            users: std::iter::once(user).collect(),
            machines: std::iter::once(machine).collect(),
        };
        let at = |owner, machine| SessionPlacement { owner, machine };
        assert!(set.hides(at(Some(user), None)));
        assert!(set.hides(at(None, Some(machine))));
        assert!(set.hides(at(Some(Uuid::new_v4()), Some(machine))));
        assert!(!set.hides(at(Some(Uuid::new_v4()), Some(Uuid::new_v4()))));
        assert!(!set.hides(at(None, None)));
        assert!(HiddenSet::default().is_empty());
    }
}
