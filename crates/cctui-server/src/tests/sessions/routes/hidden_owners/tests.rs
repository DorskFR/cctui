use std::collections::{BTreeSet, HashMap};

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use uuid::Uuid;

use cctui_proto::api::hidden_owners::HiddenOwnerKind;
use cctui_proto::ws::ServerEvent;

use super::{hide, list, unhide};
use crate::state::AppState;
use cctui_server_kernel::auth::{AuthContext, Scope};
use cctui_server_sessions::routes::sessions::{
    ListParams, SearchParams, list_sessions, search_sessions,
};

fn ctx(user_id: Uuid, admin: bool) -> AuthContext {
    let mut scopes = BTreeSet::from([Scope::Read]);
    if admin {
        scopes.insert(Scope::Admin);
    }
    AuthContext { user_id, key_id: Uuid::new_v4(), machine_id: None, scopes }
}

async fn test_state(test_name: &str) -> Option<AppState> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
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
    let user_session = session(&pool, hidden_user, user_machine, &format!("/w/{token}/u")).await;
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

async fn listed(state: &AppState, viewer: Uuid) -> HashMap<String, bool> {
    let response = list_sessions(
        State(state.clone()),
        Extension(ctx(viewer, true)),
        None,
        Query(ListParams { include_archived: false }),
        HeaderMap::new(),
    )
    .await
    .unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    flags(&serde_json::from_slice(&body).unwrap())
}

async fn searched(f: &Fixture, viewer: Uuid) -> HashMap<String, bool> {
    let Json(found) = search_sessions(
        State(f.state.clone()),
        Extension(ctx(viewer, true)),
        Query(SearchParams {
            q: f.token.clone(),
            include_archived: true,
            limit: Some(50),
            offset: None,
            cursor: None,
            exclude_dispatched: false,
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

    let mine = listed(&f.state, f.admin).await;
    assert!(!mine.contains_key(&f.user_session), "hidden user's session listed");
    assert_eq!(mine.get(&f.machine_session), Some(&false));
    let found = searched(&f, f.admin).await;
    assert!(!found.contains_key(&f.user_session), "hidden user's session found");
    assert_eq!(found.get(&f.machine_session), Some(&false));

    let theirs = listed(&f.state, f.other_admin).await;
    assert_eq!(theirs.get(&f.user_session), Some(&false), "another admin still sees it");
    assert!(searched(&f, f.other_admin).await.contains_key(&f.user_session));

    set_hidden(&f, HiddenOwnerKind::User, f.user, false).await;
    set_hidden(&f, HiddenOwnerKind::User, f.user, false).await;
    assert_eq!(listed(&f.state, f.admin).await.get(&f.user_session), Some(&false));
    assert!(searched(&f, f.admin).await.contains_key(&f.user_session));
}

#[tokio::test]
async fn hidden_machine_sessions_leave_the_admins_list_and_search_only() {
    let Some(f) = fixture("hidden_machine_sessions_leave_the_admins_list_and_search_only").await
    else {
        return;
    };
    set_hidden(&f, HiddenOwnerKind::Machine, f.machine, true).await;

    let mine = listed(&f.state, f.admin).await;
    assert!(!mine.contains_key(&f.machine_session), "hidden machine's session listed");
    assert_eq!(mine.get(&f.user_session), Some(&false));
    assert!(!searched(&f, f.admin).await.contains_key(&f.machine_session));
    assert!(listed(&f.state, f.other_admin).await.contains_key(&f.machine_session));

    set_hidden(&f, HiddenOwnerKind::Machine, f.machine, false).await;
    assert!(listed(&f.state, f.admin).await.contains_key(&f.machine_session));
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
    let Some(f) = fixture("hidden_owners_refuses_non_admins_self_and_unknown_targets").await else {
        return;
    };
    let state = || State(f.state.clone());
    let plain = || Extension(ctx(f.user, false));
    let err = list(state(), plain()).await.unwrap_err();
    assert_eq!(err.status(), StatusCode::FORBIDDEN);
    let path = || Path((HiddenOwnerKind::Machine, f.machine));
    assert_eq!(hide(state(), plain(), path()).await.unwrap_err().status(), StatusCode::FORBIDDEN);
    assert_eq!(unhide(state(), plain(), path()).await.unwrap_err().status(), StatusCode::FORBIDDEN);
    let mut machine_key = ctx(f.admin, true);
    machine_key.machine_id = Some(f.machine);
    assert_eq!(
        list(state(), Extension(machine_key)).await.unwrap_err().status(),
        StatusCode::FORBIDDEN
    );

    let admin = || Extension(ctx(f.admin, true));
    let own = Path((HiddenOwnerKind::User, f.admin));
    assert_eq!(hide(state(), admin(), own).await.unwrap_err().status(), StatusCode::BAD_REQUEST);
    let unknown = Path((HiddenOwnerKind::User, Uuid::new_v4()));
    assert_eq!(hide(state(), admin(), unknown).await.unwrap_err().status(), StatusCode::NOT_FOUND);
    let own_machine = machine(&f.state.pool, f.admin).await;
    let own_machine = Path((HiddenOwnerKind::Machine, own_machine));
    assert_eq!(hide(state(), admin(), own_machine).await.unwrap(), StatusCode::NO_CONTENT);
}
