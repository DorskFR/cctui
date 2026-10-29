use cctui_proto::adapter::AdapterId;
use chrono::Utc;
use uuid::Uuid;

use crate::state::AppState;

#[allow(clippy::too_many_arguments)]
pub(super) async fn upsert_session(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    local_id: &str,
    working_dir: Option<String>,
    parent_local_id: Option<String>,
    observed_at: Option<i64>,
    extra: Option<serde_json::Value>,
) -> anyhow::Result<Option<bool>> {
    Ok(cctui_server_core::lifecycle::upsert_session(
        &state.pool,
        &state.bus,
        machine_id,
        user_id,
        adapter_id,
        local_id,
        working_dir,
        parent_local_id,
        observed_at,
        extra,
    )
    .await?)
}

/// Register a session the daemon announced, announcing it to browser clients
/// only on a first insert: the codex inventory poll re-announces every known
/// thread every ~15 s.
pub(super) async fn register_announced_session(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    local_id: &str,
) -> anyhow::Result<()> {
    if upsert_session(state, machine_id, user_id, adapter_id, local_id, None, None, None, None)
        .await?
        == Some(true)
    {
        publish_session_registered(state, local_id).await;
        crate::events::record(
            state,
            crate::events::Event::new(
                crate::events::kind::SESSION_CREATED,
                crate::events::Actor::Daemon,
            )
            .session(local_id)
            .machine(machine_id)
            .user(user_id)
            .detail(serde_json::json!({ "origin": "daemon-registered", "adapter_id": adapter_id })),
        );
    }
    Ok(())
}

/// Tell browser clients a session exists as soon as it is registered, instead
/// of leaving them to discover it on the next list poll. Best-effort: a row
/// that cannot be read back is simply not announced.
pub(super) async fn publish_session_registered(state: &AppState, local_id: &str) {
    #[derive(sqlx::FromRow)]
    struct Row {
        id: String,
        parent_id: Option<String>,
        account_id: Option<String>,
        machine_id: String,
        working_dir: String,
        status: String,
        registered_at: chrono::DateTime<Utc>,
        last_heartbeat: chrono::DateTime<Utc>,
        metadata: serde_json::Value,
        adapter_id: Option<String>,
    }

    let row: Option<Row> = match sqlx::query_as(
        "SELECT id, parent_id, account_id, machine_id, working_dir, status, registered_at, \
                last_heartbeat, metadata, adapter_id \
           FROM sessions WHERE id = $1",
    )
    .bind(local_id)
    .fetch_optional(&state.pool)
    .await
    {
        Ok(row) => row,
        Err(err) => {
            tracing::warn!(%err, %local_id, "session read-back failed; registration not announced");
            return;
        }
    };
    let Some(row) = row else { return };
    let (status, _) = crate::routes::sessions::resolve_status_liveness(
        &row.status,
        row.registered_at,
        row.last_heartbeat,
    );
    state.bus.publish_server(cctui_proto::ws::ServerEvent::SessionRegistered {
        session: cctui_proto::models::Session {
            id: row.id,
            parent_id: row.parent_id,
            account_id: row.account_id,
            machine_id: row.machine_id,
            working_dir: row.working_dir,
            status,
            registered_at: row.registered_at,
            last_heartbeat: row.last_heartbeat,
            metadata: row.metadata,
            adapter_id: row.adapter_id.map(AdapterId::new),
        },
    });
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::routes::daemon::test_support::{drop_machines, row_version, seed_machine};

    #[tokio::test]
    async fn a_released_session_re_registers_while_an_archived_one_stays_archived() {
        let Some(url) = crate::routes::gateway::test_db_url("released_reregister") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let uid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("rel-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        let machine_id = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(machine_id)
            .bind(uid)
            .bind(format!("m-{machine_id}"))
            .bind(format!("mk-{machine_id}"))
            .execute(&pool)
            .await
            .expect("seed machine");

        let released = format!("rel-{}", uuid::Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO sessions \
                 (id, machine_id, machine_uuid, working_dir, user_id, adapter_id, \
                  status, ended_at, end_reason, end_detail) \
             VALUES ($1, $2, $3, '/w', $4, 'claude-code', 'ended', now(), 'other', \
                     'released: this claude job was not started by cctui')",
        )
        .bind(&released)
        .bind(machine_id.to_string())
        .bind(machine_id)
        .bind(uid)
        .execute(&pool)
        .await
        .expect("seed released session");

        let archived = format!("arc-{}", uuid::Uuid::new_v4().simple());
        sqlx::query(
            "INSERT INTO sessions \
                 (id, machine_id, machine_uuid, working_dir, user_id, adapter_id, status) \
             VALUES ($1, $2, $3, '/w', $4, 'claude-code', 'archived')",
        )
        .bind(&archived)
        .bind(machine_id.to_string())
        .bind(machine_id)
        .bind(uid)
        .execute(&pool)
        .await
        .expect("seed archived session");

        let state = AppState::for_test(pool.clone());
        for id in [&released, &archived] {
            super::upsert_session(
                &state,
                machine_id,
                uid,
                "claude-code",
                id,
                Some("/w".to_owned()),
                None,
                None,
                None,
            )
            .await
            .expect("upsert");
        }

        let (status, end_detail): (String, Option<String>) =
            sqlx::query_as("SELECT status, end_detail FROM sessions WHERE id = $1")
                .bind(&released)
                .fetch_one(&pool)
                .await
                .expect("read released");
        assert_eq!(status, "active", "a released job is live again and must come back");
        assert_eq!(end_detail, None);

        let status: String = sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
            .bind(&archived)
            .fetch_one(&pool)
            .await
            .expect("read archived");
        assert_eq!(status, "archived", "a roster snapshot must not un-archive a session");

        sqlx::query("DELETE FROM sessions WHERE id = ANY($1)")
            .bind(&[released, archived][..])
            .execute(&pool)
            .await
            .expect("cleanup");
        sqlx::query("DELETE FROM machines WHERE id = $1")
            .bind(machine_id)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    #[tokio::test]
    async fn an_unchanged_codex_inventory_tick_writes_nothing() {
        let Some(url) = crate::routes::gateway::test_db_url("unchanged_inventory") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "inventory").await;
        let sid = Uuid::new_v4().to_string();
        let state = AppState::for_test(pool.clone());
        let tick = || {
            upsert_session(
                &state,
                mid,
                uid,
                "codex",
                &sid,
                Some("/w".into()),
                None,
                Some(1_762_000_000),
                Some(json!({ "source": "codex-thread-list", "observed_at": 1_762_000_000 })),
            )
        };
        assert_eq!(tick().await.expect("first"), Some(true));
        let before = row_version(&pool, &sid).await;
        assert_eq!(tick().await.expect("second"), Some(false), "still owned, not new");
        assert_eq!(row_version(&pool, &sid).await, before, "the row was not rewritten");

        drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }
}
