use super::*;

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
    let Some(pool) = test_pool("an_agent_tick_after_the_users_leaves_the_users_resolution").await
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
    let got =
        tick(&state, &sid, Uuid::new_v4(), UserActionStatus::Done, None, UserActionResolver::Agent)
            .await
            .unwrap();
    assert!(matches!(got, Ticked::Missing));
}
