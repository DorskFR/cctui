use cctui_proto::ws::DaemonFrameDown;
use uuid::Uuid;

pub(super) async fn seed_session(
    pool: &sqlx::PgPool,
    adapter: &str,
    provider: &str,
) -> (uuid::Uuid, String) {
    let uid = uuid::Uuid::new_v4();
    let prov = uuid::Uuid::new_v4();
    let session_id = format!("ses_{}", uuid::Uuid::new_v4().simple());
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("meter-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .expect("seed user");
    let acct = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
        .bind(acct)
        .bind(uid)
        .bind(format!("meter-acct-{uid}"))
        .execute(pool)
        .await
        .expect("seed account");
    sqlx::query(
        "INSERT INTO account_providers \
             (id, user_id, provider, encrypted_refresh_token, account_id) \
         VALUES ($1, $2, $3, 'x', $4)",
    )
    .bind(prov)
    .bind(uid)
    .bind(provider)
    .bind(acct)
    .execute(pool)
    .await
    .expect("seed provider");
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, adapter_id) \
         VALUES ($1, 'm1', '/w', $2, $3)",
    )
    .bind(&session_id)
    .bind(uid)
    .bind(adapter)
    .execute(pool)
    .await
    .expect("seed session");
    sqlx::query(
        "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
    )
    .bind(format!("th-{session_id}"))
    .bind(&session_id)
    .bind(prov)
    .execute(pool)
    .await
    .expect("seed token");
    (uid, session_id)
}

pub(super) async fn seed_machine(pool: &sqlx::PgPool, tag: &str) -> (Uuid, Uuid) {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("{tag}-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .expect("seed user");
    let mid = Uuid::new_v4();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(mid)
        .bind(uid)
        .bind(format!("m-{mid}"))
        .bind(format!("mk-{mid}"))
        .execute(pool)
        .await
        .expect("seed machine");
    (uid, mid)
}

pub(super) async fn seed_owned_session(pool: &sqlx::PgPool, uid: Uuid, mid: Uuid) -> String {
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, user_id, adapter_id) \
         VALUES ($1, $2, $3, '/w', 'active', $4, 'claude-code')",
    )
    .bind(&sid)
    .bind(mid.to_string())
    .bind(mid)
    .bind(uid)
    .execute(pool)
    .await
    .expect("seed session");
    sid
}

pub(super) async fn drop_machines(
    pool: &sqlx::PgPool,
    sessions: &[String],
    owners: &[(Uuid, Uuid)],
) {
    sqlx::query("DELETE FROM sessions WHERE id = ANY($1)").bind(sessions).execute(pool).await.ok();
    for (uid, mid) in owners {
        sqlx::query("DELETE FROM machines WHERE id = $1").bind(mid).execute(pool).await.ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(pool).await.ok();
    }
}

pub(super) fn reply(local_id: &str) -> DaemonFrameDown {
    DaemonFrameDown::Command {
        adapter_id: "claude-code".into(),
        command: Box::new(cctui_proto::adapter::AdapterCommand::Reply {
            local_id: local_id.into(),
            text: "hi".into(),
            ask_picks: None,
            env: std::collections::BTreeMap::new(),
            command_id: None,
            turn_id: None,
        }),
    }
}

pub(super) async fn row_version(pool: &sqlx::PgPool, sid: &str) -> String {
    sqlx::query_scalar("SELECT xmin::text FROM sessions WHERE id = $1")
        .bind(sid)
        .fetch_one(pool)
        .await
        .expect("row version")
}
