use super::*;

async fn pool() -> Option<PgPool> {
    let url = crate::routes::gateway::test_db_url("presence")?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

fn prefix(tag: &str) -> String {
    format!("cct1070-{tag}-{}-", Uuid::new_v4().simple())
}

async fn wipe(pool: &PgPool, prefix: &str) {
    let like = format!("{prefix}%");
    sqlx::query("DELETE FROM pods WHERE pod LIKE $1").bind(&like).execute(pool).await.unwrap();
    sqlx::query("DELETE FROM ws_presence WHERE pod LIKE $1")
        .bind(&like)
        .execute(pool)
        .await
        .unwrap();
}

async fn ws_owner(pool: &PgPool, kind: Kind, id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT pod FROM ws_presence WHERE kind = $1 AND entity_id = $2",
    )
    .bind(kind.as_str())
    .bind(id)
    .fetch_optional(pool)
    .await
    .unwrap()
}

/// A daemon reconnects to the same pod: the old connection's close runs
/// after the new one registered. The row and its heartbeat entry belong
/// to the new connection and survive; only the new one's close drops them.
#[tokio::test]
async fn a_late_close_of_the_old_connection_keeps_the_reconnects_row() {
    let Some(pool) = pool().await else { return };
    let prefix = prefix("reconnect");
    let mut state = AppState::for_test(pool.clone());
    state.presence = std::sync::Arc::new(PodIdentity::for_test(
        &format!("{prefix}a"),
        &format!("{prefix}10.0.5.1"),
    ));
    let machine = Uuid::new_v4();
    let key = (Kind::Daemon, machine);
    let (old, new) = (Uuid::new_v4(), Uuid::new_v4());

    register_conn(&state, Kind::Daemon, machine, old).await;
    register_conn(&state, Kind::Daemon, machine, new).await;
    unregister_conn(&state, Kind::Daemon, machine, old).await;
    assert_eq!(ws_owner(&pool, Kind::Daemon, machine).await, Some(state.presence.pod.clone()));
    assert_eq!(state.presence.owned.get(&key).map(|owner| *owner), Some(new));

    sqlx::query("DELETE FROM ws_presence WHERE entity_id = $1")
        .bind(machine)
        .execute(&pool)
        .await
        .unwrap();
    heartbeat_tick(&pool, &state.presence).await;
    assert_eq!(
        ws_owner(&pool, Kind::Daemon, machine).await,
        Some(state.presence.pod.clone()),
        "the heartbeat still re-asserts the live connection's row",
    );

    unregister_conn(&state, Kind::Daemon, machine, new).await;
    assert_eq!(ws_owner(&pool, Kind::Daemon, machine).await, None);
    assert!(!state.presence.owned.contains_key(&key));
    wipe(&pool, &prefix).await;
}
