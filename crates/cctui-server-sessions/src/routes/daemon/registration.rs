use cctui_proto::adapter::AdapterId;
use chrono::Utc;
use uuid::Uuid;

use crate::context::SessionsContext;

#[allow(clippy::too_many_arguments)]
pub async fn upsert_session<S: SessionsContext>(
    state: &S,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    local_id: &str,
    working_dir: Option<String>,
    parent_local_id: Option<String>,
    observed_at: Option<i64>,
    extra: Option<serde_json::Value>,
) -> anyhow::Result<Option<bool>> {
    Ok(crate::lifecycle::upsert_session(
        state.pool(),
        state.bus(),
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
pub(super) async fn register_announced_session<S: SessionsContext>(
    state: &S,
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
pub async fn publish_session_registered<S: SessionsContext>(state: &S, local_id: &str) {
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
    .fetch_optional(state.pool())
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
    state.bus().publish_server(cctui_proto::ws::ServerEvent::SessionRegistered {
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
