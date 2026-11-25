//! Write-side checks on an item's reach, and the bulk match editor.

use axum::extract::State;
use axum::http::StatusCode;
use axum::{Extension, Json};
use sqlx::PgPool;
use sqlx::types::Json as Jsonb;
use uuid::Uuid;

use super::{
    BulkContextOutcome, BulkContextResponse, BulkContextUpdate, COLS, ContextItem, ContextItemSpec,
    ContextMatch, ContextMatchLists, ContextMatchSet, ContextReach, Row, clean_match, clean_spec,
    db_err, get_one, is_unborn_proposal, spec_of, targets,
};
use crate::auth::AuthContext;
use crate::context::SessionsContext;
use crate::error::AppError;

pub const MAX_BULK: usize = 500;

pub fn refuse_all_users(ctx: &AuthContext, m: &ContextMatch) -> Result<(), AppError> {
    if m.reach == ContextReach::AllUsers && !ctx.is_admin() {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "only an admin can make an item reach all users' machines",
        ));
    }
    Ok(())
}

pub async fn resolve_owned_spec(
    pool: &PgPool,
    ctx: &AuthContext,
    spec: ContextItemSpec,
    keep: &[&ContextMatch],
) -> Result<ContextItemSpec, AppError> {
    refuse_all_users(ctx, &spec.match_)?;
    targets::refuse_foreign_machines(pool, ctx.user_id, &spec.match_).await?;
    targets::resolve_spec(pool, ctx.user_id, spec, keep).await
}

pub fn hold_agent_reach(base: &ContextMatch, next: &mut ContextMatch) -> Result<(), AppError> {
    let widens = (next.reach == ContextReach::AllUsers && base.reach != ContextReach::AllUsers)
        || (next.dispatch && !base.dispatch);
    if widens {
        return Err(AppError::new(
            StatusCode::FORBIDDEN,
            "an agent cannot set reach or dispatch: a human sets them in Settings → Context",
        ));
    }
    keep_reach(base, next);
    Ok(())
}

fn keep_reach(base: &ContextMatch, next: &mut ContextMatch) {
    next.reach = base.reach;
    next.dispatch = base.dispatch;
    next.dispatchers.clone_from(&base.dispatchers);
    next.profiles.clone_from(&base.profiles);
}

/// What approving `item`'s proposal makes live: the proposed content under the
/// item's reach as it stands now, or the default reach for an agent's new item.
#[must_use]
pub fn approved_spec(item: &ContextItem) -> Option<ContextItemSpec> {
    let mut spec = item.pending.clone()?;
    let default = ContextMatch::default();
    let base = if is_unborn_proposal(item) { &default } else { &item.match_ };
    keep_reach(base, &mut spec.match_);
    Some(spec)
}

fn lists(l: ContextMatchLists) -> Result<ContextMatch, AppError> {
    clean_match(ContextMatch {
        machines: l.machines,
        paths: l.paths,
        labels: l.labels,
        adapters: l.adapters,
        dispatchers: l.dispatchers,
        profiles: l.profiles,
        ..ContextMatch::default()
    })
}

fn edit(list: &[String], add: &[String], remove: &[String]) -> Vec<String> {
    let mut out: Vec<String> =
        list.iter().chain(add).filter(|v| !remove.contains(v)).cloned().collect();
    out.sort();
    out.dedup();
    out
}

/// A value both added and removed is removed.
#[must_use]
pub fn apply_bulk(
    m: &ContextMatch,
    add: &ContextMatch,
    remove: &ContextMatch,
    set: &ContextMatchSet,
) -> ContextMatch {
    let machines: &[String] = if set.clear_machines { &[] } else { &m.machines };
    ContextMatch {
        machines: edit(machines, &add.machines, &remove.machines),
        paths: edit(&m.paths, &add.paths, &remove.paths),
        labels: edit(&m.labels, &add.labels, &remove.labels),
        adapters: edit(&m.adapters, &add.adapters, &remove.adapters),
        reach: set.reach.unwrap_or(m.reach),
        dispatch: set.dispatch.unwrap_or(m.dispatch),
        dispatchers: edit(&m.dispatchers, &add.dispatchers, &remove.dispatchers),
        profiles: edit(&m.profiles, &add.profiles, &remove.profiles),
    }
}

/// Body, title, tags, enabled and any pending proposal are left as they are.
pub async fn update_match(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    version: i32,
    spec: &ContextItemSpec,
) -> Result<Option<ContextItem>, sqlx::Error> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "UPDATE context_items SET filters = $4, scope = $5, scope_ref = $6, \
            updated_by_session = NULL, version = version + 1, updated_at = now() \
         WHERE id = $1 AND user_id = $2 AND version = $3 RETURNING {COLS}"
    )))
    .bind(id)
    .bind(user_id)
    .bind(version)
    .bind(Jsonb(spec.match_.clone()))
    .bind(&spec.scope)
    .bind(spec.scope_ref.as_deref())
    .fetch_optional(pool)
    .await?;
    Ok(row.map(ContextItem::from))
}

struct BulkEdit {
    add: ContextMatch,
    remove: ContextMatch,
    set: ContextMatchSet,
}

async fn bulk_one(
    pool: &PgPool,
    ctx: &AuthContext,
    id: Uuid,
    edit: &BulkEdit,
) -> Result<ContextItem, AppError> {
    let not_found = || AppError::new(StatusCode::NOT_FOUND, "context item not found");
    let item = get_one(pool, ctx.user_id, id).await?.ok_or_else(not_found)?;
    let next = ContextItemSpec {
        scope: "user".to_owned(),
        scope_ref: None,
        match_: apply_bulk(&item.match_, &edit.add, &edit.remove, &edit.set),
        ..spec_of(&item)
    };
    let spec = resolve_owned_spec(pool, ctx, clean_spec(next)?, &targets::kept(&item)).await?;
    if spec.match_ == item.match_ {
        return Ok(item);
    }
    update_match(pool, ctx.user_id, id, item.version, &spec).await.map_err(db_err)?.ok_or_else(
        || {
            AppError::new(
                StatusCode::CONFLICT,
                "the item changed during the edit; reload and retry",
            )
        },
    )
}

/// `PATCH /context` — applied directly, no proposal; one item's refusal
/// leaves the rest applied.
pub async fn bulk_update<S: SessionsContext>(
    State(state): State<S>,
    Extension(ctx): Extension<AuthContext>,
    Json(req): Json<BulkContextUpdate>,
) -> Result<Json<BulkContextResponse>, AppError> {
    let mut ids = req.ids;
    let mut seen = std::collections::HashSet::new();
    ids.retain(|id| seen.insert(*id));
    if ids.is_empty() {
        return Err(AppError::new(StatusCode::BAD_REQUEST, "ids is required"));
    }
    if ids.len() > MAX_BULK {
        return Err(AppError::new(
            StatusCode::BAD_REQUEST,
            format!("at most {MAX_BULK} items per edit"),
        ));
    }
    let wide = ContextMatch { reach: req.set.reach.unwrap_or_default(), ..ContextMatch::default() };
    refuse_all_users(&ctx, &wide)?;
    let edit = BulkEdit { add: lists(req.add)?, remove: lists(req.remove)?, set: req.set };
    let mut outcomes = Vec::with_capacity(ids.len());
    for id in ids {
        outcomes.push(match bulk_one(state.pool(), &ctx, id, &edit).await {
            Ok(item) => BulkContextOutcome { id, item: Some(item), status: None, error: None },
            Err(e) => {
                let (code, Json(body)) = e.into_parts();
                BulkContextOutcome {
                    id,
                    item: None,
                    status: Some(code.as_u16()),
                    error: Some(body.error),
                }
            }
        });
    }
    Ok(Json(BulkContextResponse { outcomes }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::Scope;
    use crate::routes::context::{insert, list_for_user};

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_owned()).collect()
    }

    fn human(user_id: Uuid, admin: bool) -> AuthContext {
        let mut scopes = vec![Scope::Read, Scope::Dispatch];
        if admin {
            scopes.push(Scope::Admin);
        }
        AuthContext {
            user_id,
            key_id: Uuid::nil(),
            machine_id: None,
            scopes: scopes.into_iter().collect(),
        }
    }

    #[test]
    fn a_bulk_edit_removes_then_adds_and_sets_only_what_it_names() {
        let m = ContextMatch {
            machines: strings(&["m-1", "m-2"]),
            paths: strings(&["/w"]),
            adapters: strings(&["codex"]),
            ..ContextMatch::default()
        };
        let add = ContextMatch { machines: strings(&["m-3", "m-2"]), ..ContextMatch::default() };
        let remove = ContextMatch { machines: strings(&["m-1"]), ..ContextMatch::default() };
        let out = apply_bulk(&m, &add, &remove, &ContextMatchSet::default());
        assert_eq!(out.machines, ["m-2", "m-3"]);
        assert_eq!((out.paths, out.adapters), (m.paths.clone(), m.adapters.clone()), "untouched");
        assert_eq!((out.reach, out.dispatch), (ContextReach::Owner, false));

        let set = ContextMatchSet {
            reach: Some(ContextReach::AllUsers),
            dispatch: Some(true),
            clear_machines: true,
        };
        let out = apply_bulk(&m, &ContextMatch::default(), &ContextMatch::default(), &set);
        assert!(out.machines.is_empty(), "back to the default reach");
        assert_eq!((out.reach, out.dispatch), (ContextReach::AllUsers, true));
        assert_eq!(out.paths, m.paths);
    }

    #[test]
    fn a_value_added_and_removed_in_one_edit_is_removed() {
        let both = ContextMatch { labels: strings(&["l"]), ..ContextMatch::default() };
        let out = apply_bulk(&ContextMatch::default(), &both, &both, &ContextMatchSet::default());
        assert!(out.labels.is_empty());
    }

    #[test]
    fn only_an_admin_may_ask_for_all_users() {
        let wide = ContextMatch { reach: ContextReach::AllUsers, ..ContextMatch::default() };
        let err = refuse_all_users(&human(Uuid::new_v4(), false), &wide).expect_err("not admin");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
        assert!(refuse_all_users(&human(Uuid::new_v4(), true), &wide).is_ok());
        assert!(refuse_all_users(&human(Uuid::new_v4(), false), &ContextMatch::default()).is_ok());
    }

    #[test]
    fn an_agent_cannot_set_reach_or_dispatch() {
        let base = ContextMatch::default();
        let mut wide = ContextMatch { reach: ContextReach::AllUsers, ..ContextMatch::default() };
        let err = hold_agent_reach(&base, &mut wide).expect_err("all users");
        assert_eq!(err.status(), StatusCode::FORBIDDEN);
        let mut dispatch = ContextMatch { dispatch: true, ..ContextMatch::default() };
        assert!(hold_agent_reach(&base, &mut dispatch).is_err(), "dispatch");

        let mut narrowing =
            ContextMatch { dispatchers: strings(&["kube"]), ..ContextMatch::default() };
        hold_agent_reach(&base, &mut narrowing).expect("narrowing alone opts in to nothing");
        assert!(narrowing.dispatchers.is_empty(), "the human's dispatch settings stand");

        let set = ContextMatch {
            reach: ContextReach::AllUsers,
            dispatch: true,
            dispatchers: strings(&["kube"]),
            ..ContextMatch::default()
        };
        let mut rewritten = ContextMatch { paths: strings(&["/w"]), ..ContextMatch::default() };
        hold_agent_reach(&set, &mut rewritten).expect("a new filter keeps the human's reach");
        assert_eq!((rewritten.reach, rewritten.dispatch), (ContextReach::AllUsers, true));
        assert_eq!(rewritten.dispatchers, ["kube"]);
        assert_eq!(rewritten.paths, ["/w"]);
    }

    #[test]
    fn an_approval_takes_the_content_and_keeps_the_live_reach() {
        let live = ContextMatch {
            reach: ContextReach::AllUsers,
            dispatch: true,
            dispatchers: strings(&["kube"]),
            ..ContextMatch::default()
        };
        let stale = ContextMatch { paths: strings(&["/w"]), ..ContextMatch::default() };
        let item = ContextItem {
            id: Uuid::new_v4(),
            user_id: Uuid::new_v4(),
            kind: "memory".into(),
            name: "note".into(),
            title: "note".into(),
            body: "live".into(),
            scope: "user".into(),
            scope_ref: None,
            match_: live,
            tags: Vec::new(),
            enabled: true,
            version: 3,
            pending: Some(ContextItemSpec {
                body: "proposed".into(),
                match_: stale,
                ..memory("note")
            }),
            created_by_session: Some("agent-1".into()),
            updated_by_session: Some("agent-1".into()),
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let spec = approved_spec(&item).expect("pending");
        assert_eq!(spec.body, "proposed");
        assert_eq!(spec.match_.paths, ["/w"], "the proposed filter");
        assert_eq!((spec.match_.reach, spec.match_.dispatch), (ContextReach::AllUsers, true));
        assert_eq!(spec.match_.dispatchers, ["kube"]);

        let mut unborn = item.clone();
        unborn.version = 1;
        unborn.enabled = false;
        unborn.body = String::new();
        let spec = approved_spec(&unborn).expect("pending");
        assert_eq!((spec.match_.reach, spec.match_.dispatch), (ContextReach::Owner, false));
        assert!(spec.match_.dispatchers.is_empty(), "an agent's new item gets the default reach");

        assert!(approved_spec(&ContextItem { pending: None, ..item }).is_none());
    }

    async fn test_pool(tag: &str) -> Option<PgPool> {
        let url = cctui_server_kernel::test_support::db_url(tag)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    async fn seed_user(pool: &PgPool) -> Uuid {
        let uid = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("ctx-bulk-{uid}"))
            .bind(format!("hctx-bulk-{uid}"))
            .execute(pool)
            .await
            .expect("seed user");
        uid
    }

    async fn seed_machine(pool: &PgPool, uid: Uuid) -> Uuid {
        let mid = Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(pool)
            .await
            .expect("seed machine");
        mid
    }

    fn memory(name: &str) -> ContextItemSpec {
        clean_spec(ContextItemSpec {
            kind: "memory".into(),
            name: name.into(),
            title: format!("T {name}"),
            body: format!("body {name}"),
            tags: strings(&["keep"]),
            match_: ContextMatch { paths: strings(&["/w"]), ..ContextMatch::default() },
            ..ContextItemSpec::default()
        })
        .expect("valid")
    }

    #[tokio::test]
    async fn a_bulk_edit_reports_per_item_and_touches_only_the_filter() {
        let Some(pool) = test_pool("context_bulk_edit").await else { return };
        let (uid, other) = (seed_user(&pool).await, seed_user(&pool).await);
        let (mine, theirs) = (seed_machine(&pool, uid).await, seed_machine(&pool, other).await);
        let mut ids = Vec::new();
        for i in 0..32 {
            ids.push(
                insert(&pool, uid, &memory(&format!("n{i:02}")), None, None).await.unwrap().id,
            );
        }
        let foreign_item = insert(&pool, other, &memory("theirs"), None, None).await.unwrap().id;
        let before = list_for_user(&pool, uid).await.unwrap();
        let ctx = human(uid, false);

        let edit = BulkEdit {
            add: lists(ContextMatchLists {
                machines: vec![mine.to_string()],
                ..Default::default()
            })
            .unwrap(),
            remove: ContextMatch::default(),
            set: ContextMatchSet { dispatch: Some(true), ..ContextMatchSet::default() },
        };
        for id in &ids {
            let item = bulk_one(&pool, &ctx, *id, &edit).await.expect("applied");
            assert_eq!(item.match_.machines, [mine.to_string()]);
            assert!(item.match_.dispatch);
        }
        let err = bulk_one(&pool, &ctx, foreign_item, &edit).await.expect_err("not theirs");
        assert_eq!(err.status(), StatusCode::NOT_FOUND);

        let bad = BulkEdit {
            add: lists(ContextMatchLists {
                machines: vec![theirs.to_string()],
                ..Default::default()
            })
            .unwrap(),
            remove: ContextMatch::default(),
            set: ContextMatchSet::default(),
        };
        let err = bulk_one(&pool, &ctx, ids[0], &bad).await.expect_err("a foreign machine");
        assert_eq!(err.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let still = bulk_one(&pool, &ctx, ids[1], &edit).await.expect("the rest still apply");
        assert_eq!(still.match_.machines, [mine.to_string()]);

        let after = list_for_user(&pool, uid).await.unwrap();
        for (b, a) in before.iter().zip(&after) {
            assert_eq!(
                (&b.title, &b.body, &b.tags, b.enabled, &b.match_.paths),
                (&a.title, &a.body, &a.tags, a.enabled, &a.match_.paths),
                "only the filter changes"
            );
            assert!(a.pending.is_none(), "no proposal");
            assert_eq!(a.scope, "user", "two keys map to no legacy scope");
        }

        let cleared = BulkEdit {
            add: ContextMatch::default(),
            remove: ContextMatch::default(),
            set: ContextMatchSet {
                clear_machines: true,
                dispatch: Some(false),
                ..Default::default()
            },
        };
        let item = bulk_one(&pool, &ctx, ids[2], &cleared).await.expect("cleared");
        assert!(item.match_.machines.is_empty());
        assert_eq!((item.scope.as_str(), item.scope_ref.as_deref()), ("path", Some("/w")));

        sqlx::query("DELETE FROM context_items WHERE user_id = ANY($1)")
            .bind(vec![uid, other])
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM machines WHERE user_id = ANY($1)")
            .bind(vec![uid, other])
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM users WHERE id = ANY($1)")
            .bind(vec![uid, other])
            .execute(&pool)
            .await
            .ok();
    }
}
