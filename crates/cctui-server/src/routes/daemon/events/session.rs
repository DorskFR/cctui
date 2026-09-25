use cctui_proto::adapter::EndReason;
use serde_json::json;
use uuid::Uuid;

use crate::live_sessions::live_sessions_predicate;
use crate::state::AppState;

/// `end_detail` cap: enough for an exit status plus a 40-line stderr tail
/// without letting a runaway log balloon the sessions row.
const END_DETAIL_MAX_BYTES: usize = 2048;

/// Trim `detail` to [`END_DETAIL_MAX_BYTES`] on a char boundary.
pub fn truncate_end_detail(detail: &str) -> &str {
    if detail.len() <= END_DETAIL_MAX_BYTES {
        return detail;
    }
    let mut end = END_DETAIL_MAX_BYTES;
    while !detail.is_char_boundary(end) {
        end -= 1;
    }
    &detail[..end]
}

pub(super) async fn mark_session_ended(
    state: &AppState,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    reason: &EndReason,
) -> anyhow::Result<()> {
    persist_session_end(&state.pool, machine_id, user_id, local_id, reason).await?;
    // Revoke any per-session gateway tokens: the session-scoped
    // cctui tokens minted at spawn map to `(session_id, account_id)` and must
    // die with the session so the gateway can no longer be driven under them.
    crate::routes::gateway::revoke_session_tokens(state, local_id).await;
    Ok(())
}

pub(super) fn publish_session_ended(state: &AppState, local_id: &str, reason: &EndReason) {
    state.bus.publish_server(cctui_proto::ws::ServerEvent::SessionEnded {
        session_id: local_id.to_owned(),
        reason: reason.kind(),
        detail: reason.detail().map(|d| truncate_end_detail(d).to_owned()),
    });
}

/// Insert the row for a spawn that never registered, then end it. `false`
/// when the id already exists (the session did start after all).
pub(super) async fn persist_failed_spawn(
    pool: &sqlx::PgPool,
    row: &crate::state::FailedSpawnRow,
    reason: &EndReason,
) -> anyhow::Result<bool> {
    let inserted: Option<(String,)> = sqlx::query_as(
        r"INSERT INTO sessions
            (id, machine_id, machine_uuid, working_dir, status, registered_at, last_heartbeat,
             metadata, user_id, adapter_id, session_name, model, effort)
          VALUES ($1, $2, $3, $4, 'ended', now(), now(), '{}'::jsonb, $5, $6, $7, $8, $9)
          ON CONFLICT (id) DO NOTHING
          RETURNING id",
    )
    .bind(&row.session_id)
    .bind(row.machine_id.to_string())
    .bind(row.machine_id)
    .bind(&row.working_dir)
    .bind(row.user_id)
    .bind(&row.adapter_id)
    .bind(&row.name)
    .bind(&row.model)
    .bind(&row.effort)
    .fetch_optional(pool)
    .await?;
    if inserted.is_none() {
        return Ok(false);
    }
    persist_session_end(pool, row.machine_id, row.user_id, &row.session_id, reason).await?;
    Ok(true)
}

/// Record the end: a `session_ended` stream event (the conversation's final
/// line) plus the row's sticky `ended` status, `ended_at`, `end_reason` and
/// `end_detail`. A no-op unless `machine_id`/`user_id` own the session.
pub(in crate::routes::daemon) async fn persist_session_end(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    reason: &EndReason,
) -> anyhow::Result<()> {
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
    // Flip to the sticky terminal status `ended` so clients render the
    // terminal state IMMEDIATELY. Plain `inactive` was re-derived back to
    // Active for ~5 min from the still-recent heartbeat (admin::derive_status
    // is time-based) — masking the end of unattended/dispatched jobs. `ended`
    // is honoured as terminal by the list/search read paths regardless of
    // heartbeat age. We do not delete the row — archival remains the
    // persistence story; un-archive/resume can revive it.
    sqlx::query(concat!(
        "UPDATE sessions SET status = 'ended', ended_at = now(), end_reason = $2, \
                 end_detail = $3 \
             WHERE id = $1 AND machine_uuid = $4 AND user_id = $5 AND ",
        live_sessions_predicate!()
    ))
    .bind(local_id)
    .bind(reason.kind().as_str())
    .bind(reason.detail().map(truncate_end_detail))
    .bind(machine_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Advance a session's stored transcript high-water mark to `offset`, keeping
/// the max so a replayed / out-of-order mark can't rewind it. Handed
/// back to the daemon as a resume point on its next connect.
pub(super) async fn update_transcript_mark(
    state: &AppState,
    local_id: &str,
    offset: u64,
) -> anyhow::Result<()> {
    let offset = i64::try_from(offset).unwrap_or(i64::MAX);
    sqlx::query(
        "UPDATE sessions SET transcript_offset = $2 WHERE id = $1 AND transcript_offset < $2",
    )
    .bind(local_id)
    .bind(offset)
    .execute(&state.pool)
    .await?;
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
}
