use cctui_proto::ws::DaemonFrameUp;
use uuid::Uuid;

use super::*;
use crate::routes::daemon::connection::session_scope;
use crate::routes::daemon::registration::upsert_session;
use crate::routes::daemon::test_support::{drop_machines, reply, seed_machine, seed_owned_session};

#[tokio::test]
async fn a_foreign_announce_cannot_take_over_a_session() {
    let Some(url) = cctui_server_kernel::test_support::db_url("foreign_announce") else {
        return;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect test db");
    let (ua, ma) = seed_machine(&pool, "owner").await;
    let (ub, mb) = seed_machine(&pool, "intruder").await;
    let sid = seed_owned_session(&pool, ua, ma).await;

    let bus = crate::bus::Bus::new(Box::new(crate::bus::NoopTransport));
    let (tx_a, mut rx_a) = tokio::sync::mpsc::channel(8);
    let (tx_b, mut rx_b) = tokio::sync::mpsc::channel(8);
    let conn_b = Uuid::new_v4();
    bus.register_daemon(ma, Uuid::new_v4(), tx_a);
    bus.register_daemon(mb, conn_b, tx_b);

    let mut intruder = SessionOwners::new(mb, ub);
    let announce = DaemonFrameUp::SessionRegistered {
        adapter_id: "claude-code".into(),
        local_id: sid.clone(),
    };
    assert_eq!(session_scope(&announce), Some(sid.as_str()));
    assert!(!admit(&mut intruder, &pool, &sid).await, "a foreign announce must be dropped");

    bus.bind_session_conn(&sid, conn_b);
    assert!(!claim_announced(&mut intruder, &pool, &bus, conn_b, &sid).await);
    let state = crate::state::AppState::for_test(pool.clone());
    let upserted = upsert_session(&state, mb, ub, "claude-code", &sid, None, None, None, None)
        .await
        .expect("upsert");
    assert_eq!(upserted, None, "the upsert must refuse a row another machine owns");
    let (machine, user): (Option<Uuid>, Option<Uuid>) =
        sqlx::query_as("SELECT machine_uuid, user_id FROM sessions WHERE id = $1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .expect("read back");
    assert_eq!((machine, user), (Some(ma), Some(ua)));

    bus.command_daemon_for_session(ma, &sid, reply(&sid)).await.expect("route");
    assert!(rx_a.recv().await.is_some(), "the owner still receives its session's frames");
    assert!(rx_b.try_recv().is_err(), "the intruder receives nothing for the session");

    drop_machines(&pool, &[sid], &[(ua, ma), (ub, mb)]).await;
}
