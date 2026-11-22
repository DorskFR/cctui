use cctui_proto::adapter::{AdapterEvent, EndReason};
use cctui_proto::features::Feature;
use serde_json::json;
use uuid::Uuid;

use crate::context::SessionsContext;
use crate::routes::daemon::registration::{publish_session_registered, upsert_session};

pub use crate::lifecycle::truncate_end_detail;

async fn mark_session_ended<S: SessionsContext>(
    state: &S,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    reason: &EndReason,
) -> anyhow::Result<()> {
    if persist_session_end(state.pool(), machine_id, user_id, local_id, reason).await? {
        record_session_ended(state, local_id, machine_id, user_id, reason);
    }
    // Session-scoped gateway tokens die with the session.
    state.revoke_session_tokens(local_id).await;
    Ok(())
}

/// One `session.ended` per row that actually flipped to `ended`.
pub(super) fn record_session_ended<S: SessionsContext>(
    state: &S,
    local_id: &str,
    machine_id: Uuid,
    user_id: Uuid,
    reason: &EndReason,
) {
    use crate::events::{Actor, Event, Severity, kind};
    use cctui_proto::models::SessionEndReason;
    let end_reason = reason.kind();
    let severity = match end_reason {
        SessionEndReason::Crashed
        | SessionEndReason::OomKilled
        | SessionEndReason::ResumeFailed
        | SessionEndReason::SpawnFailed => Severity::Error,
        SessionEndReason::DaemonLost | SessionEndReason::MachineOffline => Severity::Warn,
        SessionEndReason::Completed
        | SessionEndReason::Killed
        | SessionEndReason::ReapedInactive
        | SessionEndReason::Other => Severity::Info,
    };
    crate::events::record(
        state,
        Event::new(kind::SESSION_ENDED, Actor::Daemon)
            .severity(severity)
            .session(local_id)
            .machine(machine_id)
            .user(user_id)
            .detail(json!({
                "end_reason": end_reason.as_str(),
                "end_detail": reason.detail().map(truncate_end_detail),
            })),
    );
}

pub fn publish_session_ended<S: SessionsContext>(state: &S, local_id: &str, reason: &EndReason) {
    state.bus().publish_server(cctui_proto::ws::ServerEvent::SessionEnded {
        session_id: local_id.to_owned(),
        reason: reason.kind(),
        detail: reason.detail().map(|d| truncate_end_detail(d).to_owned()),
    });
}

/// `WHERE EXISTS` guard: a `session_ended` can arrive for a session the server
/// never registered (its `SessionStarted` was dropped, or an ephemeral subagent
/// session); a bare INSERT trips `stream_events_session_id_fkey`.
const SESSION_ENDED_SQL: &str = "INSERT INTO stream_events (session_id, event_type, payload) \
     SELECT $1, 'session_ended', $2 WHERE EXISTS ( \
         SELECT 1 FROM sessions WHERE id = $1 AND machine_uuid = $3 AND user_id = $4) \
     ON CONFLICT (session_id, event_type, content_hash, \
                  COALESCE(turn_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
     DO NOTHING";

struct TranscriptJournal;

impl crate::lifecycle::SessionEndJournal for TranscriptJournal {
    async fn before_end(
        &self,
        pool: &sqlx::PgPool,
        machine_id: Uuid,
        user_id: Uuid,
        local_id: &str,
        reason: &EndReason,
    ) -> Result<(), sqlx::Error> {
        let payload = json!({ "reason": reason });
        sqlx::query(SESSION_ENDED_SQL)
            .bind(local_id)
            .bind(&payload)
            .bind(machine_id)
            .bind(user_id)
            .execute(pool)
            .await?;
        Ok(())
    }
}

pub async fn persist_failed_spawn(
    pool: &sqlx::PgPool,
    row: &crate::commands::FailedSpawnRow,
    reason: &EndReason,
) -> anyhow::Result<bool> {
    let journal = TranscriptJournal;
    Ok(crate::lifecycle::persist_failed_spawn(pool, &journal, row, reason).await?)
}

pub async fn persist_session_end(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    reason: &EndReason,
) -> anyhow::Result<bool> {
    Ok(crate::lifecycle::persist_session_end(
        pool,
        &TranscriptJournal,
        machine_id,
        user_id,
        local_id,
        reason,
    )
    .await?)
}

use crate::daemon_checkpoints::{ack_transcript_mark, update_transcript_mark};

/// Session lifecycle events: start, end, transcript mark and model.
pub async fn on_session_event<S: SessionsContext>(
    state: &S,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    event: AdapterEvent,
) -> anyhow::Result<()> {
    match event {
        e @ AdapterEvent::SessionStarted { .. } => {
            on_session_started(state, machine_id, user_id, adapter_id, e).await?;
        }
        AdapterEvent::SessionEnded { local_id, reason } => {
            mark_session_ended(state, machine_id, user_id, &local_id, &reason).await?;
            crate::guard_step::forget(&local_id);
            publish_session_ended(state, &local_id, &reason);
            state.preview_close_session(&local_id).await;
        }
        AdapterEvent::TranscriptMark { local_id, offset } => {
            update_transcript_mark(state, &local_id, offset).await?;
            ack_transcript_mark(state, machine_id, adapter_id, &local_id, offset).await;
        }
        AdapterEvent::SessionModel { local_id, model } => {
            // Claude Code's local API-error messages report the placeholder
            // `<synthetic>`; storing it would break a later `--model` resume.
            // Older daemons still send it, so the server filters too.
            let Some(model) = cctui_proto::adapter::observed_model_id(&model) else {
                tracing::debug!(%local_id, model, "ignoring placeholder session model");
                return Ok(());
            };
            // Transcript ground truth overrides the requested `--model`.
            sqlx::query("UPDATE sessions SET model = $2 WHERE id = $1")
                .bind(&local_id)
                .bind(model)
                .execute(state.pool())
                .await
                .map_err(|e| {
                    tracing::error!("db error (session model): {e}");
                    e
                })?;
        }
        _ => {}
    }
    Ok(())
}

/// `spawn_key` is attacker-chosen data on a machine-authenticated event, so
/// neither the rebind nor the claim of the intents filed under the key runs
/// until the key, the target id and the credentials filed under the key all
/// belong to the announcing machine's user.
async fn rebind_permitted<S: SessionsContext>(
    state: &S,
    machine_id: Uuid,
    user_id: Uuid,
    spawn_key: &str,
    local_id: &str,
) -> bool {
    if !cctui_server_kernel::accounts_types::needs_rebind(spawn_key, local_id) {
        return false;
    }
    match crate::store::tokens::rebind_allowed(
        state.pool(),
        spawn_key,
        local_id,
        machine_id,
        user_id,
    )
    .await
    {
        Ok(true) => true,
        Ok(false) => {
            tracing::warn!(
                %machine_id,
                %spawn_key,
                session = %local_id,
                "refusing a spawn-key rebind that does not belong to this machine's user"
            );
            false
        }
        Err(e) => {
            tracing::error!(%spawn_key, session = %local_id, error = %e, "spawn-key rebind check failed");
            false
        }
    }
}

async fn on_session_started<S: SessionsContext>(
    state: &S,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    event: AdapterEvent,
) -> anyhow::Result<()> {
    let AdapterEvent::SessionStarted { local_id, meta } = event else {
        return Ok(());
    };
    let working_dir = meta.working_dir.clone();
    let observed_at = meta.extra.get("observed_at").and_then(serde_json::Value::as_i64);
    let extra = (!meta.extra.is_null()).then(|| meta.extra.clone());
    let spawn_key_hint = if let Some(spawn_key) =
        meta.extra.get("spawn_key").and_then(serde_json::Value::as_str)
        && rebind_permitted(state, machine_id, user_id, spawn_key, &local_id).await
    {
        state
            .rebind_spawn_key(
                cctui_proto::ids::SpawnKey::from(spawn_key),
                cctui_proto::ids::SessionId::from(local_id.as_str()),
            )
            .await;
        Some(spawn_key.to_owned())
    } else {
        None
    };
    let Some(first_registration) = upsert_session(
        state,
        machine_id,
        user_id,
        adapter_id,
        &local_id,
        working_dir,
        meta.parent_local_id.clone(),
        observed_at,
        extra,
    )
    .await?
    else {
        return Ok(());
    };
    if first_registration {
        publish_session_registered(state, &local_id).await;
        let origin = match (&spawn_key_hint, &meta.parent_local_id) {
            (Some(_), _) => "spawned",
            (None, Some(_)) => "child",
            (None, None) => "daemon-registered",
        };
        crate::events::record(
            state,
            crate::events::Event::new(
                crate::events::kind::SESSION_CREATED,
                crate::events::Actor::Daemon,
            )
            .session(&local_id)
            .machine(machine_id)
            .user(user_id)
            .detail(json!({
                "origin": origin,
                "adapter_id": adapter_id,
                "parent_session_id": meta.parent_local_id,
            })),
        );
    }
    state
        .machine_launches_registered(
            spawn_key_hint.as_deref().unwrap_or(&local_id),
            &local_id,
            machine_id,
            user_id,
        )
        .await?;
    crate::auto_archive::claim_intent(state, &local_id, spawn_key_hint.as_deref()).await;
    let features = state.config().features;
    if features.contains(Feature::Organising) {
        crate::spawn_labels::claim_intent(state.pool(), &local_id, spawn_key_hint.as_deref()).await;
    }
    if features.contains(Feature::Automation) {
        crate::followup::claim_intent(state.pool(), &local_id, spawn_key_hint.as_deref()).await;
    }
    claim_context(state, &local_id, spawn_key_hint.as_deref()).await;
    state.claim_spawner(&local_id, spawn_key_hint.as_deref()).await;
    if let Some(context_key) = meta.extra.get("context_key").and_then(serde_json::Value::as_str) {
        claim_fork_context(state, &local_id, context_key, user_id).await;
        state.claim_spawner(&local_id, Some(context_key)).await;
    }
    Ok(())
}

async fn claim_context<S: SessionsContext>(state: &S, session_id: &str, spawn_key: Option<&str>) {
    if state.config().features.contains(Feature::Context) {
        crate::routes::context::claim_intent(state.pool(), session_id, spawn_key).await;
    } else if let Some(spawn_key) = spawn_key {
        crate::routes::context::ownership::inherit(state.pool(), session_id, spawn_key, None).await;
    }
}

async fn claim_fork_context<S: SessionsContext>(
    state: &S,
    session_id: &str,
    context_key: &str,
    user_id: Uuid,
) {
    if state.config().features.contains(Feature::Context) {
        crate::routes::context::claim_fork_intent(state.pool(), session_id, context_key, user_id)
            .await;
    } else {
        crate::routes::context::ownership::inherit(
            state.pool(),
            session_id,
            context_key,
            Some(user_id),
        )
        .await;
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;
    use crate::routes::daemon::test_support::{seed_machine, seed_owned_session};

    #[test]
    fn end_detail_truncates_on_char_boundary() {
        let short = "exit status: 1";
        assert_eq!(super::truncate_end_detail(short), short);
        let long = "é".repeat(2000);
        let cut = super::truncate_end_detail(&long);
        assert!(cut.len() <= 2048);
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[tokio::test]
    async fn persist_failed_spawn_writes_an_ended_row_once() {
        let Some(url) = cctui_server_kernel::test_support::db_url("persist_failed_spawn") else {
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
            .bind(format!("spawn-{uid}"))
            .bind(format!("kh-{uid}"))
            .execute(&pool)
            .await
            .expect("seed user");
        let mid = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
            .bind(mid)
            .bind(uid)
            .bind(format!("m-{mid}"))
            .bind(format!("mk-{mid}"))
            .execute(&pool)
            .await
            .expect("seed machine");
        let row = crate::commands::FailedSpawnRow {
            session_id: uuid::Uuid::new_v4().to_string(),
            machine_id: mid,
            user_id: uid,
            adapter_id: "codex".into(),
            working_dir: "/w".into(),
            name: Some("nope".into()),
            model: Some("gpt-nope".into()),
            effort: None,
        };
        let reason = EndReason::SpawnFailed {
            detail: "unknown model gpt-nope; available: gpt-5-codex".into(),
        };
        assert!(super::persist_failed_spawn(&pool, &row, &reason).await.expect("persist"));
        assert!(
            !super::persist_failed_spawn(&pool, &row, &reason).await.expect("persist again"),
            "an existing row is left alone"
        );

        let (status, end_reason, end_detail, model, name): (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        ) = sqlx::query_as(
            "SELECT status, end_reason, end_detail, model, session_name FROM sessions WHERE id = $1",
        )
        .bind(&row.session_id)
        .fetch_one(&pool)
        .await
        .expect("read row");
        assert_eq!(status, "ended");
        assert_eq!(end_reason.as_deref(), Some("spawn_failed"));
        assert_eq!(end_detail.as_deref(), Some("unknown model gpt-nope; available: gpt-5-codex"));
        assert_eq!(model.as_deref(), Some("gpt-nope"));
        assert_eq!(name.as_deref(), Some("nope"));
    }

    #[tokio::test]
    async fn persist_session_end_fills_reason_columns() {
        let Some(url) = cctui_server_kernel::test_support::db_url("persist_session_end") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "end").await;
        let sid = seed_owned_session(&pool, uid, mid).await;
        sqlx::query("UPDATE sessions SET tempo = 'active', agent_state = 'working' WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .expect("mid-turn signals");

        let detail =
            format!("claude -p exited (exit status: 1); last stderr:\n{}", "x".repeat(3000));
        let reason = EndReason::Crashed { detail: detail.clone() };
        super::persist_session_end(&pool, mid, uid, &sid, &reason).await.expect("persist");

        let (status, end_reason, end_detail, ended_at): (
            String,
            Option<String>,
            Option<String>,
            Option<chrono::DateTime<Utc>>,
        ) = sqlx::query_as(
            "SELECT status, end_reason, end_detail, ended_at FROM sessions WHERE id = $1",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .expect("read back");
        assert_eq!(status, "ended");
        assert_eq!(end_reason.as_deref(), Some("crashed"));
        let end_detail = end_detail.expect("detail persisted");
        assert!(detail.starts_with(&end_detail));
        assert_eq!(end_detail.len(), 2048);
        assert!(ended_at.is_some());
        let signals: (Option<String>, Option<String>) =
            sqlx::query_as("SELECT tempo, agent_state FROM sessions WHERE id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .expect("read signals");
        assert_eq!(signals, (None, Some("stopped".to_owned())), "an ended row is not busy");

        let events: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM stream_events WHERE session_id = $1 AND event_type = 'session_ended'",
        )
        .bind(&sid)
        .fetch_one(&pool)
        .await
        .expect("count events");
        assert_eq!(events, 1);

        super::persist_session_end(&pool, mid, uid, &sid, &EndReason::Killed)
            .await
            .expect("persist");
        let (end_reason, end_detail): (Option<String>, Option<String>) =
            sqlx::query_as("SELECT end_reason, end_detail FROM sessions WHERE id = $1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .expect("read back");
        assert_eq!(end_reason.as_deref(), Some("killed"));
        assert_eq!(end_detail, None);
    }
}
