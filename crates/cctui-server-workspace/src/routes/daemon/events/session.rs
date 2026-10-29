use cctui_proto::adapter::{AdapterEvent, EndReason};
use serde_json::json;
use uuid::Uuid;

use crate::routes::daemon::registration::{publish_session_registered, upsert_session};
use crate::state::AppState;

pub use cctui_server_core::lifecycle::truncate_end_detail;

async fn mark_session_ended(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    reason: &EndReason,
) -> anyhow::Result<()> {
    if persist_session_end(&state.pool, machine_id, user_id, local_id, reason).await? {
        record_session_ended(state, local_id, machine_id, user_id, reason);
    }
    // Session-scoped gateway tokens die with the session.
    crate::routes::gateway::revoke_session_tokens(state, local_id).await;
    Ok(())
}

/// One `session.ended` per row that actually flipped to `ended`.
pub(super) fn record_session_ended(
    state: &AppState,
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

pub(super) fn publish_session_ended(state: &AppState, local_id: &str, reason: &EndReason) {
    state.bus.publish_server(cctui_proto::ws::ServerEvent::SessionEnded {
        session_id: local_id.to_owned(),
        reason: reason.kind(),
        detail: reason.detail().map(|d| truncate_end_detail(d).to_owned()),
    });
}

struct TranscriptJournal;
impl cctui_server_core::lifecycle::SessionEndJournal for TranscriptJournal {
    async fn before_end(
        &self,
        pool: &sqlx::PgPool,
        machine_id: Uuid,
        user_id: Uuid,
        local_id: &str,
        reason: &EndReason,
    ) -> Result<(), sqlx::Error> {
        let payload = json!({ "reason": reason });
        // `WHERE EXISTS` guard: a session_ended can arrive for a session the server
        // never registered (its SessionStarted was dropped, or an ephemeral subagent
        // session) — a bare INSERT trips `stream_events_session_id_fkey`. No-op
        // cleanly instead of erroring; the UPDATE below is already missing-safe.
        sqlx::query(
            "INSERT INTO stream_events (session_id, event_type, payload) \
         SELECT $1, 'session_ended', $2 WHERE EXISTS ( \
             SELECT 1 FROM sessions WHERE id = $1 AND machine_uuid = $3 AND user_id = $4) \
         ON CONFLICT (session_id, event_type, content_hash, \
                      COALESCE(turn_id, '00000000-0000-0000-0000-000000000000'::uuid)) \
         DO NOTHING",
        )
        .bind(local_id)
        .bind(&payload)
        .bind(machine_id)
        .bind(user_id)
        .execute(pool)
        .await?;
        Ok(())
    }
}

pub(super) async fn persist_failed_spawn(
    pool: &sqlx::PgPool,
    row: &crate::state::FailedSpawnRow,
    reason: &EndReason,
) -> anyhow::Result<bool> {
    Ok(cctui_server_core::lifecycle::persist_failed_spawn(pool, &TranscriptJournal, row, reason)
        .await?)
}

pub(in crate::routes::daemon) async fn persist_session_end(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    reason: &EndReason,
) -> anyhow::Result<bool> {
    Ok(cctui_server_core::lifecycle::persist_session_end(
        pool,
        &TranscriptJournal,
        machine_id,
        user_id,
        local_id,
        reason,
    )
    .await?)
}

use cctui_server_core::daemon_checkpoints::{ack_transcript_mark, update_transcript_mark};

/// Session lifecycle events: start, end, transcript mark and model.
pub(super) async fn on_session_event(
    state: &AppState,
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
            crate::preview::close_session(state, &local_id).await;
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
                .execute(&state.pool)
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
async fn rebind_permitted(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    spawn_key: &str,
    local_id: &str,
) -> bool {
    if !crate::routes::gateway::needs_rebind(spawn_key, local_id) {
        return false;
    }
    match crate::store::tokens::rebind_allowed(
        &state.pool,
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

async fn on_session_started(
    state: &AppState,
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
        crate::routes::gateway::rebind_spawn_key(
            state,
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
    cctui_server_core::machine_launches::registered(
        &state.pool,
        spawn_key_hint.as_deref().unwrap_or(&local_id),
        &local_id,
        machine_id,
        user_id,
    )
    .await?;
    crate::auto_archive::claim_intent(state, &local_id, spawn_key_hint.as_deref()).await;
    crate::spawn_labels::claim_intent(&state.pool, &local_id, spawn_key_hint.as_deref()).await;
    crate::followup::claim_intent(&state.pool, &local_id, spawn_key_hint.as_deref()).await;
    crate::routes::context::claim_intent(&state.pool, &local_id, spawn_key_hint.as_deref()).await;
    crate::routes::spawn::claim_spawner(&state.pool, &local_id, spawn_key_hint.as_deref()).await;
    if let Some(context_key) = meta.extra.get("context_key").and_then(serde_json::Value::as_str) {
        crate::routes::context::claim_fork_intent(&state.pool, &local_id, context_key, user_id)
            .await;
        crate::routes::spawn::claim_spawner(&state.pool, &local_id, Some(context_key)).await;
    }
    Ok(())
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

    /// A `<synthetic>` model (Claude Code's local API-error messages) never
    /// overwrites the recorded model; a real id still does.
    #[tokio::test]
    async fn session_model_ignores_synthetic_placeholder() {
        let Some(url) = crate::routes::gateway::test_db_url("session_model_synthetic") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, mid) = seed_machine(&pool, "session-model-synthetic").await;
        let sid = seed_owned_session(&pool, uid, mid).await;
        let model_of = |sid: String| {
            let pool = pool.clone();
            async move {
                sqlx::query_scalar::<_, Option<String>>("SELECT model FROM sessions WHERE id = $1")
                    .bind(sid)
                    .fetch_one(&pool)
                    .await
                    .expect("read model")
            }
        };
        let send = |model: &str| {
            super::on_session_event(
                &state,
                mid,
                uid,
                "claude-code",
                AdapterEvent::SessionModel { local_id: sid.clone(), model: model.to_owned() },
            )
        };

        send("deepseek-v4-flash").await.expect("real model");
        assert_eq!(model_of(sid.clone()).await.as_deref(), Some("deepseek-v4-flash"));
        for placeholder in ["<synthetic>", "  ", ""] {
            send(placeholder).await.expect("placeholder model");
            assert_eq!(
                model_of(sid.clone()).await.as_deref(),
                Some("deepseek-v4-flash"),
                "{placeholder:?} must not overwrite the model"
            );
        }

        crate::routes::daemon::test_support::drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }

    #[tokio::test]
    async fn a_session_end_forgets_its_guard_step() {
        let Some(url) = crate::routes::gateway::test_db_url("session_end_guard_step") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, mid) = seed_machine(&pool, "session-end-guard").await;
        let sid = seed_owned_session(&pool, uid, mid).await;
        crate::guard_step::record(
            sid.clone(),
            cctui_proto::guard_step::GuardStepState { step: 2, ..Default::default() },
        );

        super::on_session_event(
            &state,
            mid,
            uid,
            "claude-code",
            AdapterEvent::SessionEnded { local_id: sid.clone(), reason: EndReason::Completed },
        )
        .await
        .expect("session end");
        assert_eq!(crate::guard_step::get(&sid), None);

        crate::routes::daemon::test_support::drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }

    #[tokio::test]
    async fn persist_failed_spawn_writes_an_ended_row_once() {
        let Some(url) = crate::routes::gateway::test_db_url("persist_failed_spawn") else {
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
        let row = crate::state::FailedSpawnRow {
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

    /// A guard-less dispatch's daemon ends its session `completed` as the
    /// worker shuts down; arriving after a transport mark, it replaces it.
    #[tokio::test]
    async fn a_completed_end_after_a_transport_mark_upgrades_it() {
        let Some(url) = crate::routes::gateway::test_db_url("session_end_after_mark") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, mid) = seed_machine(&pool, "end-after-mark").await;
        let mut sids = Vec::new();
        for mark in ["daemon_lost", "machine_offline"] {
            let sid = seed_owned_session(&pool, uid, mid).await;
            sqlx::query(
                "UPDATE sessions SET status = 'ended', ended_at = now(), end_reason = $2, \
                 end_detail = 'transport' WHERE id = $1",
            )
            .bind(&sid)
            .bind(mark)
            .execute(&pool)
            .await
            .expect("mark");

            super::on_session_event(
                &state,
                mid,
                uid,
                "claude-code",
                AdapterEvent::SessionEnded { local_id: sid.clone(), reason: EndReason::Completed },
            )
            .await
            .expect("session end");

            let (status, end_reason, end_detail): (String, Option<String>, Option<String>) =
                sqlx::query_as("SELECT status, end_reason, end_detail FROM sessions WHERE id = $1")
                    .bind(&sid)
                    .fetch_one(&pool)
                    .await
                    .expect("read back");
            assert_eq!(
                (status.as_str(), end_reason.as_deref(), end_detail),
                ("ended", Some("completed"), None),
                "{mark} is replaced by the reported completion"
            );
            sids.push(sid);
        }

        crate::routes::daemon::test_support::drop_machines(&pool, &sids, &[(uid, mid)]).await;
    }

    #[tokio::test]
    async fn persist_session_end_fills_reason_columns() {
        let Some(url) = crate::routes::gateway::test_db_url("persist_session_end") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let (uid, mid) = seed_machine(&pool, "end").await;
        let sid = seed_owned_session(&pool, uid, mid).await;

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

    /// An account named `Astra-<id>` with one openai provider row, for `uid`.
    async fn seed_openai_account(pool: &sqlx::PgPool, uid: Uuid) -> (Uuid, Uuid) {
        let acct = Uuid::new_v4();
        let prov = Uuid::new_v4();
        sqlx::query("INSERT INTO accounts (id, user_id, name) VALUES ($1, $2, $3)")
            .bind(acct)
            .bind(uid)
            .bind(format!("Astra-{acct}"))
            .execute(pool)
            .await
            .expect("seed account");
        sqlx::query(
            "INSERT INTO account_providers \
                 (id, user_id, provider, encrypted_refresh_token, account_id) \
             VALUES ($1, $2, 'openai', 'x', $3)",
        )
        .bind(prov)
        .bind(uid)
        .bind(acct)
        .execute(pool)
        .await
        .expect("seed provider");
        (acct, prov)
    }

    /// DB-gated: a codex spawn binds its gateway token to the dispatch key
    /// (codex mints its own thread id), and the thread registers under that
    /// thread id echoing the key as `spawn_key`. The token, and the spawn's
    /// attachments, must land on the thread id: every account lookup of the
    /// session list (`account_name`, switch-account, revocation at end) reads
    /// `session_tokens` by the registered id.
    #[tokio::test]
    async fn codex_start_rebinds_the_dispatch_key_onto_the_thread_id() {
        let Some(url) = crate::routes::gateway::test_db_url("codex_start_rebinds") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, mid) = seed_machine(&pool, "codex-rebind").await;
        let (acct, prov) = seed_openai_account(&pool, uid).await;
        let dispatch_key = Uuid::new_v4().to_string();
        let thread_id = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(format!("th-{dispatch_key}"))
        .bind(&dispatch_key)
        .bind(prov)
        .execute(&pool)
        .await
        .expect("seed token under the dispatch key");
        sqlx::query(
            "INSERT INTO session_attachments (session_id, name, hash, size) \
             VALUES ($1, 'brief.md', 'h', 1)",
        )
        .bind(&dispatch_key)
        .execute(&pool)
        .await
        .expect("seed bootstrap attachment");

        super::on_session_event(
            &state,
            mid,
            uid,
            "codex",
            AdapterEvent::SessionStarted {
                local_id: thread_id.clone(),
                meta: cctui_proto::adapter::SessionMeta {
                    working_dir: Some("/w".into()),
                    parent_local_id: None,
                    extra: json!({ "source": "codex-app-server", "spawn_key": dispatch_key }),
                },
            },
        )
        .await
        .expect("session started");

        let name: Option<String> = sqlx::query_scalar(
            "SELECT a.name FROM session_tokens st \
             JOIN account_providers ap ON ap.id = st.account_id \
             JOIN accounts a ON a.id = ap.account_id \
             WHERE st.session_id = $1 AND st.revoked_at IS NULL",
        )
        .bind(&thread_id)
        .fetch_optional(&pool)
        .await
        .expect("account lookup");
        assert_eq!(name, Some(format!("Astra-{acct}")), "the thread id must resolve its account");
        let left: i64 = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM session_tokens WHERE session_id = $1) \
                  + (SELECT count(*) FROM session_attachments WHERE session_id = $1)",
        )
        .bind(&dispatch_key)
        .fetch_one(&pool)
        .await
        .expect("count leftovers");
        assert_eq!(left, 0, "nothing may stay behind on the dispatch key");
        let attachments: i64 =
            sqlx::query_scalar("SELECT count(*) FROM session_attachments WHERE session_id = $1")
                .bind(&thread_id)
                .fetch_one(&pool)
                .await
                .expect("count attachments");
        assert_eq!(attachments, 1, "the bootstrap attachment follows the session");

        sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
            .bind(&thread_id)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM account_providers WHERE id = $1")
            .bind(prov)
            .execute(&pool)
            .await
            .ok();
        sqlx::query("DELETE FROM accounts WHERE id = $1").bind(acct).execute(&pool).await.ok();
        crate::routes::daemon::test_support::drop_machines(&pool, &[thread_id], &[(uid, mid)])
            .await;
    }

    async fn announce(
        state: &AppState,
        (uid, mid): (Uuid, Uuid),
        sid: &str,
        parent: Option<&str>,
        relation: serde_json::Value,
    ) {
        let meta = cctui_proto::adapter::SessionMeta {
            working_dir: Some("/w".to_owned()),
            parent_local_id: parent.map(str::to_owned),
            extra: json!({ "short": "s", "relation": relation }),
        };
        super::on_session_event(
            state,
            mid,
            uid,
            "claude-code",
            AdapterEvent::SessionStarted { local_id: sid.to_owned(), meta },
        )
        .await
        .expect("session started");
    }

    async fn relation_of(pool: &sqlx::PgPool, sid: &str) -> (Option<String>, Option<String>) {
        sqlx::query_as("SELECT parent_id, metadata->>'relation' FROM sessions WHERE id = $1")
            .bind(sid)
            .fetch_one(pool)
            .await
            .expect("read relation")
    }

    async fn followup_intent(pool: &sqlx::PgPool, sid: &str, parent: &str) {
        sqlx::query(
            "INSERT INTO session_followup_intents (spawn_key, parent_session_id) VALUES ($1, $2)",
        )
        .bind(sid)
        .bind(parent)
        .execute(pool)
        .await
        .expect("seed follow-up intent");
    }

    /// A daemon calls every session it discovers a root and re-announces it on
    /// each reconnect, before or after the follow-up claim, with `"root"` or
    /// (codex) `null`. Only a genuine root ends up a root.
    #[tokio::test]
    async fn a_root_announcement_never_takes_a_born_relation_back() {
        let Some(url) = crate::routes::gateway::test_db_url("sticky_relation") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let owner = seed_machine(&pool, "sticky-relation").await;
        let parent = seed_owned_session(&pool, owner.0, owner.1).await;
        let id = || Uuid::new_v4().to_string();
        let linked = |relation: &str| (Some(parent.clone()), Some(relation.to_owned()));

        let root_first = id();
        followup_intent(&pool, &root_first, &parent).await;
        announce(&state, owner, &root_first, None, json!("root")).await;
        assert_eq!(relation_of(&pool, &root_first).await, linked("followup"));

        let claim_first = id();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, working_dir, status, user_id, adapter_id) \
             VALUES ($1, $2, $3, '/w', 'active', $4, 'claude-code')",
        )
        .bind(&claim_first)
        .bind(owner.1.to_string())
        .bind(owner.1)
        .bind(owner.0)
        .execute(&pool)
        .await
        .expect("seed claimed session");
        followup_intent(&pool, &claim_first, &parent).await;
        crate::followup::claim_intent(&pool, &claim_first, None).await;
        announce(&state, owner, &claim_first, None, json!("root")).await;
        assert_eq!(relation_of(&pool, &claim_first).await, linked("followup"));

        let fork = id();
        announce(&state, owner, &fork, Some(parent.as_str()), json!("fork")).await;
        let late_fork = id();
        announce(&state, owner, &late_fork, None, json!("root")).await;
        announce(&state, owner, &late_fork, Some(parent.as_str()), json!("fork")).await;
        let child = id();
        announce(&state, owner, &child, Some(parent.as_str()), json!("subagent")).await;

        for sid in [&root_first, &claim_first, &fork, &late_fork, &child] {
            announce(&state, owner, sid, None, json!("root")).await;
            announce(&state, owner, sid, None, serde_json::Value::Null).await;
        }
        for sid in [&root_first, &claim_first] {
            assert_eq!(relation_of(&pool, sid).await, linked("followup"), "{sid} re-announced");
        }
        assert_eq!(relation_of(&pool, &fork).await, linked("fork"));
        assert_eq!(relation_of(&pool, &late_fork).await, linked("fork"));
        assert_eq!(relation_of(&pool, &child).await, linked("subagent"));

        let genuine = id();
        announce(&state, owner, &genuine, None, json!("root")).await;
        announce(&state, owner, &genuine, None, json!("root")).await;
        assert_eq!(relation_of(&pool, &genuine).await, (None, Some("root".to_owned())));

        let all = [root_first, claim_first, fork, late_fork, child, genuine, parent];
        crate::routes::daemon::test_support::drop_machines(&pool, &all, &[owner]).await;
    }

    /// A daemon reconnecting mid-dispatch re-announces a session whose turn
    /// already ended: the upsert keeps it live without reopening anything, the
    /// re-sent Status lands, and a completed end survives a later re-announce.
    #[tokio::test]
    async fn a_reannounce_of_a_finished_turn_keeps_its_state_and_its_end() {
        let Some(url) = crate::routes::gateway::test_db_url("reannounce_finished_turn") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");
        let state = AppState::for_test(pool.clone());
        let (uid, mid) = seed_machine(&pool, "reannounce-finished").await;
        let sid = seed_owned_session(&pool, uid, mid).await;
        let status = |tempo: &str, agent_state: &str| AdapterEvent::Status {
            local_id: sid.clone(),
            tempo: Some(tempo.to_owned()),
            state: Some(agent_state.to_owned()),
            detail: None,
            activity: None,
            name: None,
            intent: None,
            model: None,
            effort: None,
            permission_mode: None,
            children: Vec::new(),
        };
        let reannounce = || AdapterEvent::SessionStarted {
            local_id: sid.clone(),
            meta: cctui_proto::adapter::SessionMeta {
                working_dir: Some("/w".into()),
                ..Default::default()
            },
        };
        let row = || {
            let pool = pool.clone();
            let sid = sid.clone();
            async move {
                sqlx::query_as::<_, (String, Option<String>, Option<String>, Option<String>)>(
                    "SELECT status, agent_state, tempo, end_reason FROM sessions WHERE id = $1",
                )
                .bind(sid)
                .fetch_one(&pool)
                .await
                .expect("read session")
            }
        };
        let send = |event| super::on_session_event(&state, mid, uid, "claude-code", event);

        super::super::status::on_status_event(&state, status("idle", "working"))
            .await
            .expect("stale status");
        send(reannounce()).await.expect("re-announce");
        let (st, agent_state, tempo, _) = row().await;
        assert_eq!(st, "active");
        assert_eq!((agent_state.as_deref(), tempo.as_deref()), (Some("working"), Some("idle")));

        super::super::status::on_status_event(&state, status("idle", "done"))
            .await
            .expect("re-sent status");
        assert_eq!(row().await.1.as_deref(), Some("done"), "the edge's Status reconciles");

        send(AdapterEvent::SessionEnded { local_id: sid.clone(), reason: EndReason::Completed })
            .await
            .expect("end");
        send(reannounce()).await.expect("late re-announce");
        let (st, _, _, end_reason) = row().await;
        assert_eq!(st, "ended", "a completed end is not revived by a re-announce");
        assert_eq!(end_reason.as_deref(), Some("completed"));

        crate::routes::daemon::test_support::drop_machines(&pool, &[sid], &[(uid, mid)]).await;
    }
}
