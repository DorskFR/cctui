use super::*;
use crate::events::tests::{seed_session, seed_user_machine};
use cctui_server_kernel::state_events::{Actor, Event, kind, record_now};

async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

fn ctx(uid: Uuid, admin: bool) -> Extension<AuthContext> {
    let mut scopes: std::collections::BTreeSet<cctui_server_kernel::auth::Scope> =
        std::iter::once(cctui_server_kernel::auth::Scope::Read).collect();
    if admin {
        scopes.insert(cctui_server_kernel::auth::Scope::Admin);
    }
    Extension(AuthContext { user_id: uid, key_id: uid, machine_id: None, scopes })
}

/// Every route shows a user only their own rows; system rows need admin.
#[tokio::test]
async fn a_user_cannot_read_another_users_events_on_any_route() {
    let Some(pool) = test_pool("events_api_isolation").await else { return };
    let state = AppState::for_test(pool.clone());
    let (alice, alice_m) = seed_user_machine(&pool, "ev-api-a").await;
    let (bob, bob_m) = seed_user_machine(&pool, "ev-api-b").await;
    let alice_s = seed_session(&pool, alice, alice_m, Some("alice work")).await;
    let bob_s = seed_session(&pool, bob, bob_m, Some("bob secret")).await;
    record_now(&state, Event::new(kind::SESSION_KILLED, Actor::User(alice)).session(&alice_s))
        .await
        .unwrap();
    record_now(&state, Event::new(kind::SESSION_KILLED, Actor::User(bob)).session(&bob_s))
        .await
        .unwrap();
    record_now(&state, Event::new(kind::MACHINE_OFFLINE, Actor::System).machine(bob_m))
        .await
        .unwrap();
    record_now(
        &state,
        Event::new(kind::SYSTEM_ACCOUNT_LIMIT_REACHED, Actor::System).session(&bob_s),
    )
    .await
    .unwrap();

    let Json(page) =
        list_events(State(state.clone()), ctx(alice, false), Query(EventQuery::default()))
            .await
            .unwrap();
    assert!(page.events.iter().all(|e| e.user_id == Some(alice)), "{:?}", page.events);
    assert!(page.events.iter().any(|e| e.session_id.as_deref() == Some(alice_s.as_str())));
    assert!(!page.events.iter().any(|e| e.summary.contains("bob secret")));

    let Json(page) = session_events(
        State(state.clone()),
        ctx(alice, false),
        Path(bob_s.clone()),
        Query(EventQuery::default()),
    )
    .await
    .unwrap();
    assert!(page.events.is_empty(), "the route guard 403s first; the filter must also hide it");

    let Json(page) = machine_events(
        State(state.clone()),
        ctx(alice, false),
        Path(bob_m.to_string()),
        Query(EventQuery::default()),
    )
    .await
    .unwrap();
    assert!(page.events.is_empty());

    let Json(page) = session_events(
        State(state.clone()),
        ctx(bob, false),
        Path(bob_s.clone()),
        Query(EventQuery::default()),
    )
    .await
    .unwrap();
    assert_eq!(page.events.len(), 1, "system.* is hidden from the owner too: {:?}", page.events);
    assert_eq!(page.events[0].kind, kind::SESSION_KILLED);

    let Json(page) = session_events(
        State(state),
        ctx(bob, true),
        Path(bob_s.clone()),
        Query(EventQuery::default()),
    )
    .await
    .unwrap();
    assert_eq!(page.events.len(), 2, "an admin sees the system row as well");
}

#[tokio::test]
async fn keyset_pages_walk_newest_first_without_overlap() {
    let Some(pool) = test_pool("events_api_keyset").await else { return };
    let state = AppState::for_test(pool.clone());
    let (uid, machine) = seed_user_machine(&pool, "ev-api-page").await;
    let sid = seed_session(&pool, uid, machine, Some("pager")).await;
    for _ in 0..5 {
        record_now(&state, Event::new(kind::SESSION_RESUMED, Actor::User(uid)).session(&sid))
            .await
            .unwrap();
    }
    let page = |before: Option<i64>| {
        let state = state.clone();
        let sid = sid.clone();
        async move {
            let Json(page) = session_events(
                State(state),
                ctx(uid, false),
                Path(sid),
                Query(EventQuery { before, limit: Some(2), ..Default::default() }),
            )
            .await
            .unwrap();
            page
        }
    };
    let first = page(None).await;
    assert_eq!(first.events.len(), 2);
    assert!(first.has_more);
    assert!(first.events[0].id > first.events[1].id);
    let second = page(Some(first.events[1].id)).await;
    assert_eq!(second.events.len(), 2);
    assert!(second.events[0].id < first.events[1].id);
    let third = page(Some(second.events[1].id)).await;
    assert_eq!(third.events.len(), 1);
    assert!(!third.has_more);
}

#[tokio::test]
async fn kind_prefix_and_severity_filters_apply() {
    let Some(pool) = test_pool("events_api_filters").await else { return };
    let state = AppState::for_test(pool.clone());
    let (uid, machine) = seed_user_machine(&pool, "ev-api-filter").await;
    let sid = seed_session(&pool, uid, machine, Some("filtered")).await;
    record_now(&state, Event::new(kind::SESSION_KILLED, Actor::User(uid)).session(&sid))
        .await
        .unwrap();
    record_now(
        &state,
        Event::new(kind::MACHINE_STALE, Actor::System)
            .severity(cctui_server_kernel::state_events::Severity::Warn)
            .machine(machine),
    )
    .await
    .unwrap();
    let run = |q: EventQuery| {
        let state = state.clone();
        async move {
            let Json(page) = list_events(State(state), ctx(uid, false), Query(q)).await.unwrap();
            page.events
        }
    };
    let only_machine = run(EventQuery {
        kind: Some("machine.".into()),
        machine_id: Some(machine),
        ..Default::default()
    })
    .await;
    assert!(only_machine.iter().all(|e| e.kind.starts_with("machine.")));
    assert!(only_machine.iter().any(|e| e.kind == kind::MACHINE_STALE));
    let warns = run(EventQuery {
        severity: Some("warn".into()),
        machine_id: Some(machine),
        ..Default::default()
    })
    .await;
    assert!(warns.iter().all(|e| e.severity == "warn"));
    assert!(!warns.is_empty());
    let none = run(EventQuery {
        severity: Some("loud".into()),
        machine_id: Some(machine),
        ..Default::default()
    })
    .await;
    assert!(none.is_empty(), "an unknown severity selects nothing");
}
