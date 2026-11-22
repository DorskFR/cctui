use axum::extract::State;
use axum::{Extension, Json};
use cctui_server_dispatch::machine_resources::list;
use uuid::Uuid;

#[tokio::test]
async fn a_freshly_seen_machine_without_a_live_socket_is_not_connected() {
    use cctui_proto::models::MachineLiveness;
    use cctui_server_kernel::auth::{AuthContext, Scope};
    use std::collections::BTreeSet;

    let Some(url) = crate::routes::gateway::test_db_url("machine_resources_connected") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = Uuid::new_v4();
    let machine = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(owner)
        .bind(format!("resource-conn-{owner}"))
        .bind(format!("resource-conn-key-{owner}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO machines (id, user_id, name, key_hash, kind, last_seen_at) \
         VALUES ($1, $2, $3, $4, 'persistent', now())",
    )
    .bind(machine)
    .bind(owner)
    .bind(format!("machine-{machine}"))
    .bind(format!("machine-key-{machine}"))
    .execute(&pool)
    .await
    .unwrap();

    let state = crate::state::AppState::for_test(pool.clone());
    let row = || {
        let state = state.clone();
        async move {
            let ctx = AuthContext {
                user_id: owner,
                key_id: Uuid::new_v4(),
                machine_id: None,
                scopes: BTreeSet::<Scope>::new(),
            };
            let Json(rows) = list(State(state), Extension(ctx)).await.unwrap();
            rows.into_iter().find(|r| r.machine_id == machine).expect("machine listed")
        }
    };

    let offline_socket = row().await;
    assert_eq!(offline_socket.liveness, MachineLiveness::Online);
    assert!(!offline_socket.connected, "a recent heartbeat is not a live socket");

    let peer = format!("resource-peer-{machine}");
    sqlx::query(
        "INSERT INTO ws_presence (kind, entity_id, pod, pod_ip) \
         VALUES ('daemon', $1, $2, '10.0.0.9')",
    )
    .bind(machine)
    .bind(&peer)
    .execute(&pool)
    .await
    .unwrap();
    assert!(row().await.connected, "a live peer replica holds the socket");
    sqlx::query("UPDATE ws_presence SET heartbeat_at = now() - interval '1 hour' WHERE pod = $1")
        .bind(&peer)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!row().await.connected, "a stale peer row is not a live socket");

    let (tx, _rx) = tokio::sync::mpsc::channel::<cctui_proto::ws::DaemonFrameDown>(1);
    state.bus.register_daemon(machine, Uuid::new_v4(), tx);
    assert!(row().await.connected, "this replica holds the socket");

    sqlx::query("DELETE FROM ws_presence WHERE pod = $1").bind(&peer).execute(&pool).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(owner).execute(&pool).await.unwrap();
}

#[tokio::test]
async fn resource_list_scopes_owners_and_hides_revoked_users() {
    use cctui_server_kernel::auth::{AuthContext, Scope};
    use std::collections::BTreeSet;

    let Some(url) = crate::routes::gateway::test_db_url("machine_resources_owner_scope") else {
        return;
    };
    let pool = sqlx::PgPool::connect(&url).await.expect("connect test db");
    let owner = Uuid::new_v4();
    let other = Uuid::new_v4();
    let revoked = Uuid::new_v4();
    for (id, name, is_revoked) in [
        (owner, "resource-owner", false),
        (other, "resource-other", false),
        (revoked, "resource-revoked", true),
    ] {
        sqlx::query(
            "INSERT INTO users (id, name, key_hash, revoked_at) \
             VALUES ($1, $2, $3, CASE WHEN $4 THEN now() END)",
        )
        .bind(id)
        .bind(format!("{name}-{id}"))
        .bind(format!("resource-key-{id}"))
        .bind(is_revoked)
        .execute(&pool)
        .await
        .unwrap();
    }

    let seed_machine = |user_id: Uuid, kind: &'static str, revoked: bool, deleted: bool| {
        let pool = pool.clone();
        async move {
            let id = Uuid::new_v4();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash, kind, revoked_at, deleted_at) \
                 VALUES ($1, $2, $3, $4, $5, CASE WHEN $6 THEN now() END, CASE WHEN $7 THEN now() END)",
            )
            .bind(id)
            .bind(user_id)
            .bind(format!("machine-{id}"))
            .bind(format!("machine-key-{id}"))
            .bind(kind)
            .bind(revoked)
            .bind(deleted)
            .execute(&pool)
            .await
            .unwrap();
            id
        }
    };
    let own_machine = seed_machine(owner, "persistent", false, false).await;
    let other_machine = seed_machine(other, "persistent", false, false).await;
    let revoked_owner_machine = seed_machine(revoked, "persistent", false, false).await;
    let revoked_machine = seed_machine(owner, "persistent", true, false).await;
    let deleted_machine = seed_machine(owner, "persistent", false, true).await;
    let ephemeral_machine = seed_machine(owner, "ephemeral", false, false).await;

    let state = crate::state::AppState::for_test(pool.clone());
    let caller = |user_id, admin| {
        Extension(AuthContext {
            user_id,
            key_id: Uuid::new_v4(),
            machine_id: None,
            scopes: if admin { BTreeSet::from([Scope::Admin]) } else { BTreeSet::new() },
        })
    };
    let Json(own_rows) = list(State(state.clone()), caller(owner, false)).await.unwrap();
    let Json(admin_rows) = list(State(state), caller(owner, true)).await.unwrap();
    let ids = |rows: Vec<cctui_server_dispatch::machine_resources::MachineResourcesRow>| {
        let mut ids: Vec<_> = rows.into_iter().map(|row| row.machine_id).collect();
        ids.sort_unstable();
        ids
    };
    assert_eq!(own_rows.len(), 1);
    assert_eq!(own_rows[0].machine_id, own_machine);
    assert!(own_rows[0].resources.is_none(), "machines without a snapshot remain listed");
    let admin_ids = ids(admin_rows);
    for visible in [own_machine, other_machine] {
        assert!(admin_ids.contains(&visible));
    }
    for hidden in [revoked_owner_machine, revoked_machine, deleted_machine, ephemeral_machine] {
        assert!(!admin_ids.contains(&hidden));
    }
    sqlx::query("DELETE FROM users WHERE id = ANY($1)")
        .bind(vec![owner, other, revoked])
        .execute(&pool)
        .await
        .unwrap();
}
