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
use crate::context::SessionsContext;
use crate::error::AppError;

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

pub async fn list<S: SessionsContext>(
    State(state): State<S>,
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
    .fetch_all(state.pool())
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

fn changed<S: SessionsContext>(state: &S, viewer: Uuid) {
    invalidate(viewer);
    state.bus().publish_server(ServerEvent::HiddenOwnersChanged { user_id: viewer });
}

pub async fn hide<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Path((kind, target)): Path<(HiddenOwnerKind, Uuid)>,
) -> Result<StatusCode, AppError> {
    let viewer = viewer(&ctx)?;
    if kind == HiddenOwnerKind::User && target == viewer {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "you cannot hide your own sessions"));
    }
    if !target_exists(state.pool(), kind, target).await? {
        return Err(AppError::new(StatusCode::NOT_FOUND, "no such user or machine"));
    }
    let inserted = sqlx::query(
        "INSERT INTO user_hidden_owners (viewer_id, kind, target_id) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(viewer)
    .bind(kind.as_str())
    .bind(target)
    .execute(state.pool())
    .await?
    .rows_affected();
    if inserted > 0 {
        changed(&state, viewer);
    }
    Ok(StatusCode::NO_CONTENT)
}

pub async fn unhide<S: SessionsContext>(
    State(state): State<S>,
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
    .execute(state.pool())
    .await?
    .rows_affected();
    if removed > 0 {
        changed(&state, viewer);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {

    use axum::extract::Query;

    use uuid::Uuid;

    use super::{HiddenSet, SessionPlacement};

    use crate::routes::sessions::{ListParams, SearchParams};

    #[test]
    fn an_older_clients_show_hidden_is_accepted_and_ignored() {
        let list: axum::http::Uri =
            "/sessions?include_archived=true&show_hidden=true".parse().unwrap();
        let Query(params) = Query::<ListParams>::try_from_uri(&list).unwrap();
        assert!(params.include_archived);
        let search: axum::http::Uri = "/sessions/search?q=x&show_hidden=true".parse().unwrap();
        let Query(params) = Query::<SearchParams>::try_from_uri(&search).unwrap();
        assert_eq!(params.q, "x");
        assert!(!params.exclude_dispatched);
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
