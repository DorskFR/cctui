use cctui_proto::adapter::AdapterEvent;
use uuid::Uuid;

use super::Inserted;
use crate::routes::daemon::ingest::{Stored, insert_event, note_insert};
use crate::state::AppState;

pub(super) use cctui_server_core::daemon_usage::on_token_usage;
#[cfg(test)]
use cctui_server_core::daemon_usage::{gateway_metered_adapter_patterns, insert_token_usage};

/// Persist a `Message`/`ToolUse` row unless the batched ingest already did.
pub(super) async fn on_stream_event(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    event: AdapterEvent,
    stored: Stored,
) -> anyhow::Result<Inserted> {
    let seq = match event {
        AdapterEvent::Message { local_id, mut payload, turn_id } => {
            if let Stored::Done(seq) = stored {
                seq
            } else {
                crate::keepalive::observe_message(state, &local_id, &mut payload).await;
                insert_event(
                    &state.pool,
                    machine_id,
                    user_id,
                    &local_id,
                    "message",
                    payload,
                    turn_id,
                )
                .await?
            }
        }
        AdapterEvent::ToolUse { local_id, payload } => {
            if let Stored::Done(seq) = stored {
                seq
            } else {
                insert_event(&state.pool, machine_id, user_id, &local_id, "tool_use", payload, None)
                    .await?
            }
        }
        _ => return Ok(Inserted::UNSTORED),
    };
    let inserted = Inserted::from_seq(seq);
    note_insert(state, machine_id, inserted.newly);
    Ok(inserted)
}

#[cfg(test)]
mod tests {
    use crate::routes::daemon::test_support::seed_session;

    /// DB-gated: an opencode session is metered by the gateway capture alone —
    /// the daemon's model-NULL row for the same message must be a no-op, or
    /// budgets double-count. One row survives, and it carries a model.
    #[test]
    fn the_gateway_metered_patterns_name_the_metered_harnesses_and_nothing_anthropic() {
        let patterns = super::gateway_metered_adapter_patterns();
        assert_eq!(patterns, vec!["opencode%".to_owned(), "gemini%".to_owned()]);
        for own_usage in ["claude-code", "codex", "qwen", "goose"] {
            assert!(
                !patterns.iter().any(|p| own_usage.starts_with(p.trim_end_matches('%'))),
                "{own_usage}"
            );
        }
    }

    #[tokio::test]
    async fn opencode_daemon_usage_is_suppressed_gateway_is_single_source() {
        let Some(url) = crate::routes::gateway::test_db_url("opencode_daemon_usage_is_suppressed")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, session_id) = seed_session(&pool, "opencode", "fireworks").await;

        crate::routes::gateway::record_metered_usage(
            pool.clone(),
            crate::routes::gateway::Family::Fireworks,
            session_id.clone(),
            Some("accounts/fireworks/models/kimi-k3".to_owned()),
            crate::cost::CapturedUsage {
                message_id: Some("gw-1".to_owned()),
                usage: crate::cost::TokenUsage { input: 100, cached_input: 0, output: 10 },
            },
        )
        .await;

        super::insert_token_usage(&pool, &session_id, "daemon-1", 100, 10, 0, 0)
            .await
            .expect("daemon insert must not error, only no-op");

        let (rows, nulls): (i64, i64) = sqlx::query_as(
            "SELECT count(*), count(*) FILTER (WHERE model IS NULL) \
             FROM session_token_usage WHERE session_id = $1",
        )
        .bind(&session_id)
        .fetch_one(&pool)
        .await
        .expect("count rows");
        assert_eq!(rows, 1, "opencode must be single-metered by the gateway capture");
        assert_eq!(nulls, 0, "the surviving row must carry the gateway-stamped model");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }

    /// DB-gated: claude-code/codex are NOT gateway-proxied, so their single
    /// daemon-reported row must still land — the opencode gate must not suppress
    /// them.
    #[tokio::test]
    async fn non_opencode_daemon_usage_is_recorded() {
        let Some(url) =
            crate::routes::gateway::test_db_url("non_opencode_daemon_usage_is_recorded")
        else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, session_id) = seed_session(&pool, "claude-code", "anthropic").await;

        super::insert_token_usage(&pool, &session_id, "daemon-1", 100, 10, 0, 0)
            .await
            .expect("daemon insert");

        let rows: i64 =
            sqlx::query_scalar("SELECT count(*) FROM session_token_usage WHERE session_id = $1")
                .bind(&session_id)
                .fetch_one(&pool)
                .await
                .expect("count rows");
        assert_eq!(rows, 1, "a non-opencode session keeps its single daemon-metered row");

        sqlx::query("DELETE FROM users WHERE id = $1").bind(uid).execute(&pool).await.ok();
    }
}
