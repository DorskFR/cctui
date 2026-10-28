//! Essential session registration and terminal-state persistence.
use crate::bus::Bus;
use crate::commands::FailedSpawnRow;
use crate::live_sessions_predicate;
use cctui_proto::adapter::EndReason;
use uuid::Uuid;

/// SQL: the stored metadata merged with the announced one. The relation a
/// session was born with (follow-up, fork, subagent) outlives a later
/// announcement that calls it a root or nulls it.
macro_rules! merged_metadata {
    () => {
        "(COALESCE(sessions.metadata, '{}'::jsonb) || COALESCE(EXCLUDED.metadata, '{}'::jsonb) \
          || CASE WHEN COALESCE(sessions.metadata->>'relation', 'root') <> 'root' \
                   AND COALESCE(EXCLUDED.metadata->>'relation', 'root') = 'root' \
                  THEN jsonb_build_object('relation', sessions.metadata->'relation') \
                  ELSE '{}'::jsonb END)"
    };
}

#[allow(clippy::too_many_arguments)]
pub async fn upsert_session(
    pool: &sqlx::PgPool,
    bus: &Bus,
    machine_id: Uuid,
    user_id: Uuid,
    adapter_id: &str,
    local_id: &str,
    working_dir: Option<String>,
    parent_local_id: Option<String>,
    observed_at: Option<i64>,
    extra: Option<serde_json::Value>,
) -> Result<Option<bool>, sqlx::Error> {
    // `parent_id`: resolve via a subquery rather than binding the
    // raw value so a not-yet-known parent yields NULL instead of an FK
    // violation that would drop the whole insert. In the normal case the
    // parent is upserted earlier in the same poll, so it resolves. On
    // conflict, COALESCE keeps an already-set parent and otherwise fills it
    // in from a later poll.
    //
    // `status` on conflict: the codex thread/list inventory poll
    // re-emits SessionStarted for every machine-wide thread every
    // ~15s, including ones the user archived. Preserve terminal/parked states
    // (`inactive`, `archived`, `ended`) so a re-discovery refreshes the
    // heartbeat without resurrecting the session into the Working list;
    // otherwise revive it to `active`.
    let inserted: Option<bool> = sqlx::query_scalar(concat!(
        r"INSERT INTO sessions
            (id, parent_id, account_id, machine_id, working_dir, status, registered_at,
             last_heartbeat, metadata, user_id, machine_uuid, adapter_id)
          VALUES ($1, (SELECT id FROM sessions WHERE id = $7), NULL, $2, $3, 'active',
                  COALESCE(to_timestamp($8::double precision), now()),
                  COALESCE(to_timestamp($8::double precision), now()),
                  COALESCE($9::jsonb, '{}'::jsonb), $4, $5, $6)
          ON CONFLICT (id) DO UPDATE SET
            last_heartbeat = GREATEST(sessions.last_heartbeat,
                                      COALESCE(to_timestamp($8::double precision), now())),
            status = CASE WHEN sessions.status IN ('inactive', 'archived', 'ended') THEN sessions.status ELSE 'active' END,
            adapter_id = EXCLUDED.adapter_id,
            parent_id = COALESCE(sessions.parent_id, EXCLUDED.parent_id),
            metadata = ",
        merged_metadata!(),
        r"
          WHERE sessions.machine_uuid = EXCLUDED.machine_uuid
            AND sessions.user_id = EXCLUDED.user_id
            AND (sessions.last_heartbeat, sessions.status, sessions.adapter_id,
                 sessions.parent_id, sessions.metadata)
                IS DISTINCT FROM
                (GREATEST(sessions.last_heartbeat,
                          COALESCE(to_timestamp($8::double precision), now())),
                 CASE WHEN sessions.status IN ('inactive', 'archived', 'ended') THEN sessions.status ELSE 'active' END,
                 EXCLUDED.adapter_id,
                 COALESCE(sessions.parent_id, EXCLUDED.parent_id),
                 ",
        merged_metadata!(),
        r")
          RETURNING (xmax = 0)",
    ))
    .bind(local_id)
    .bind(machine_id.to_string())
    .bind(working_dir.unwrap_or_default())
    .bind(user_id)
    .bind(machine_id)
    .bind(adapter_id)
    .bind(parent_local_id)
    .bind(observed_at)
    .bind(extra)
    .fetch_optional(pool)
    .await?;
    let inserted = match inserted {
        Some(inserted) => inserted,
        None if session_owned(pool, machine_id, user_id, local_id).await? => false,
        None => {
            tracing::warn!(%machine_id, %local_id, "refusing to register a session another machine owns");
            return Ok(None);
        }
    };
    // A daemon that re-registers the session after a reconnect proves the
    // `daemon_lost` / `machine_offline` end was spurious. So is a "released"
    // end: 0.17.0 ended every claude job cctui had not started, and those jobs
    // are alive and registered again.
    let revived: Option<Option<String>> = sqlx::query_scalar(
        "WITH prior AS (SELECT end_reason FROM sessions WHERE id = $1) \
         UPDATE sessions SET status = 'active', ended_at = NULL, end_reason = NULL, end_detail = NULL \
         WHERE id = $1 AND status = 'ended' \
           AND (end_reason IN ('daemon_lost', 'machine_offline') OR end_detail LIKE 'released:%') \
         RETURNING (SELECT end_reason FROM prior)",
    )
    .bind(local_id)
    .fetch_optional(pool)
    .await?;
    if let Some(prior_end_reason) = revived {
        crate::events::record(
            pool,
            bus,
            crate::events::Event::new(
                crate::events::kind::SESSION_RESUMED,
                crate::events::Actor::Daemon,
            )
            .session(local_id)
            .machine(machine_id)
            .user(user_id)
            .detail(serde_json::json!({
                "origin": "daemon-reregistered",
                "prior_end_reason": prior_end_reason,
            })),
        );
    }
    // Repair the durable account binding: the dispatch path mints the
    // gateway token BEFORE the daemon registers the session, so mint-time's
    // best-effort `UPDATE sessions SET account_id` hit no row and the binding
    // silently stayed NULL — leaving resume-after-revocation with
    // nothing to re-mint from. Backfill it here from the newest token row
    // (live preferred). No-op for already-bound or never-bound sessions.
    sqlx::query(
        "UPDATE sessions SET account_id = t.account_id \
         FROM (SELECT st.account_id::text AS account_id FROM session_tokens st \
                WHERE st.session_id = $1 \
                ORDER BY (st.revoked_at IS NULL) DESC, st.created_at DESC LIMIT 1) t \
         WHERE sessions.id = $1 AND sessions.account_id IS NULL",
    )
    .bind(local_id)
    .execute(pool)
    .await?;
    Ok(Some(inserted))
}

/// Whether `machine_id`/`user_id` own the session row: an upsert that changed
/// nothing returns no row, the same as one refused for ownership.
async fn session_owned(
    pool: &sqlx::PgPool,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
) -> sqlx::Result<bool> {
    sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions \
                        WHERE id = $1 AND machine_uuid = $2 AND user_id = $3)",
    )
    .bind(local_id)
    .bind(machine_id)
    .bind(user_id)
    .fetch_one(pool)
    .await
}

/// `end_detail` cap: enough for an exit status plus a 40-line stderr tail
/// without letting a runaway log balloon the sessions row.
const END_DETAIL_MAX_BYTES: usize = 2048;

/// Trim `detail` to [`END_DETAIL_MAX_BYTES`] on a char boundary.
#[must_use]
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

/// Optional transcript persistence supplied explicitly by a server host.
///
/// The full host writes the conversation end record before changing row status.
/// A host without a transcript archive uses [`NoopSessionEndJournal`].
pub trait SessionEndJournal: Sync {
    fn before_end(
        &self,
        pool: &sqlx::PgPool,
        machine_id: Uuid,
        user_id: Uuid,
        local_id: &str,
        reason: &EndReason,
    ) -> impl std::future::Future<Output = Result<(), sqlx::Error>> + Send;
}

pub struct NoopSessionEndJournal;
impl SessionEndJournal for NoopSessionEndJournal {
    async fn before_end(
        &self,
        _pool: &sqlx::PgPool,
        _machine_id: Uuid,
        _user_id: Uuid,
        _local_id: &str,
        _reason: &EndReason,
    ) -> Result<(), sqlx::Error> {
        Ok(())
    }
}

/// Insert the row for a spawn that never registered, then end it. `false`
/// when the id already exists (the session did start after all).
pub async fn persist_failed_spawn(
    pool: &sqlx::PgPool,
    journal: &impl SessionEndJournal,
    row: &FailedSpawnRow,
    reason: &EndReason,
) -> Result<bool, sqlx::Error> {
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
    persist_session_end(pool, journal, row.machine_id, row.user_id, &row.session_id, reason)
        .await?;
    Ok(true)
}

/// Record an owned session's terminal state after its host journal accepts it.
///
/// Journal failures propagate before the row update. Ownership predicates,
/// archived-row handling and end-detail truncation are shared by both hosts.
pub async fn persist_session_end(
    pool: &sqlx::PgPool,
    journal: &impl SessionEndJournal,
    machine_id: Uuid,
    user_id: Uuid,
    local_id: &str,
    reason: &EndReason,
) -> Result<bool, sqlx::Error> {
    journal.before_end(pool, machine_id, user_id, local_id, reason).await?;
    // `ended` is sticky: read paths treat it as terminal regardless of
    // heartbeat age, whereas `inactive` would be re-derived to Active from a
    // still-recent heartbeat. Resume can revive the row.
    let flipped = sqlx::query(concat!(
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
    .await?
    .rows_affected();
    Ok(flipped > 0)
}
