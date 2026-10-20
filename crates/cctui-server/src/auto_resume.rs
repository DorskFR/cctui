//! Auto-resume after a connection loss or recoverable gateway auth failure (opt-in via the
//! `autoResumeOnConnectionLoss` setting).
//!
//! A Claude Code worker that loses the API mid-reply writes `API Error:
//! Connection lost mid-response…` and ends its turn without retrying. Each
//! reaper tick finds sessions whose latest assistant message is that error and
//! nudges them with "continue", backing off 1, 2 and 4 minutes; after the third
//! nudge the session is left alone and reported through ntfy.
//!
//! Each nudge carries a timestamp so the `stream_events` dedup index does not
//! swallow a repeated identical "continue".

use std::time::Duration as StdDuration;

use cctui_proto::adapter::is_gateway_auth_error;
use cctui_proto::backoff::Backoff;
use chrono::{DateTime, Duration, Utc};

use crate::live_sessions::live_sessions_predicate;
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;

/// Delay before the first nudge, then between successive nudges: 1 min doubling
/// to a 10 min cap, jittered so a server restart does not nudge every stuck
/// session at once. The delay after the final attempt is also the grace period
/// before the row is declared exhausted.
const fn schedule() -> Backoff {
    Backoff::new(StdDuration::from_mins(1), StdDuration::from_mins(10))
}

/// Nudges sent before giving up.
pub const MAX_ATTEMPTS: i32 = 3;

/// Errors older than this are ignored: a freshly deployed server must not
/// wake every session that ended on a connection loss weeks ago.
const LOOKBACK_SECS: i64 = 6 * 3600;

/// Rows examined per sweep. The reaper runs every 30 s, so a backlog drains
/// quickly without one sweep monopolising the pool.
const BATCH: i64 = 50;

/// The error family Claude Code writes when a stream is cut. Only transport
/// failures are listed: an error the model would hit again on retry (invalid
/// request, context too long, billing) must not be nudged.
const MARKERS: &[&str] = &[
    "connection lost",
    "server error mid-response",
    "the response stopped arriving",
    "stalled before a response",
    "went to sleep",
];

/// Whether an assistant message is one of Claude Code's transport-error
/// notices, i.e. the reply was cut and a plain "continue" is the right fix.
#[must_use]
pub fn is_connection_loss(text: &str) -> bool {
    let t = text.trim_start();
    if !t.starts_with("API Error:") {
        return false;
    }
    let lower = t.to_lowercase();
    MARKERS.iter().any(|m| lower.contains(m))
}

/// The nudge itself. The timestamp makes every nudge a distinct transcript
/// line and lets a human reading the session see it was automatic.
#[must_use]
pub fn resume_prompt(now: DateTime<Utc>, attempt: i32) -> String {
    format!(
        "[cctui auto-resume {} attempt {attempt}/{MAX_ATTEMPTS}] The connection to the API was \
         lost mid-response and your previous reply was cut short. Continue from where you left \
         off.",
        now.format("%Y-%m-%dT%H:%M:%SZ")
    )
}

/// What a sweep does with one stuck session.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Not due yet (or already given up on).
    Skip,
    /// Send nudge number `attempt` (1-based).
    Fire { attempt: i32 },
    /// Every nudge was sent and the session is still stuck.
    Exhaust,
}

/// Decide the action for a stuck session, from the tracked row (if any) and
/// the error currently at the tail of the transcript. Pure, so it is tested
/// without a database.
///
/// * `tracked_event_id` / `attempts` / `next_attempt_at` / `exhausted` describe
///   the `session_auto_resume` row, or `None` when the session was never
///   tracked.
/// * `error_event_id` / `error_at` describe the error message found now.
///
/// A new error alone is not progress: repeated failures share the same budget.
/// The caller clears `tracked` only after an intervening successful reply/tool.
#[must_use]
pub fn plan(
    tracked: Option<(i64, i32, DateTime<Utc>, bool)>,
    _error_event_id: i64,
    error_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Action {
    let (attempts, due) = match tracked {
        Some((_, attempts, next_attempt_at, exhausted)) => {
            if exhausted {
                return Action::Skip;
            }
            (attempts, next_attempt_at)
        }
        _ => (0, error_at + Duration::seconds(first_delay_secs())),
    };
    if now < due {
        return Action::Skip;
    }
    if attempts >= MAX_ATTEMPTS { Action::Exhaust } else { Action::Fire { attempt: attempts + 1 } }
}

/// Delay from the error to the first nudge. Un-jittered on purpose: [`plan`] is
/// re-evaluated every sweep and must reach the same verdict each time, and each
/// session's own `error_at` already spreads these out.
fn first_delay_secs() -> i64 {
    i64::try_from(schedule().peek().as_secs()).unwrap_or(60)
}

/// Seconds to wait after nudge number `attempt` (1-based) before the next
/// decision point. Jittered, and written once to `next_attempt_at`, so sessions
/// that failed together stop nudging in lockstep.
#[must_use]
pub fn backoff_after(attempt: i32) -> i64 {
    schedule().delay_secs_for(u32::try_from(attempt).unwrap_or(u32::MAX))
}

/// The `event_type`/`role`/`text` triple in `last_err` is also the predicate of
/// the partial index `idx_stream_events_api_error`: reword one side and the
/// planner stops using the index.
const STUCK_SELECT: &str = concat!(
    "WITH error_candidates AS ( \
        SELECT e.session_id, e.id, e.created_at, e.payload->>'text' AS text \
        FROM stream_events e \
        WHERE e.event_type = 'message' \
          AND e.payload->>'role' = 'assistant' \
          AND e.payload->>'text' LIKE 'API Error:%' \
          AND e.created_at >= now() - ($1 || ' seconds')::interval \
        UNION ALL \
        SELECT e.session_id, e.id, e.created_at, e.payload->>'text' AS text \
        FROM stream_events e \
        WHERE e.event_type = 'message' \
          AND e.payload->>'role' = 'assistant' \
          AND e.payload->>'text' LIKE 'Please run /login%' \
          AND e.created_at >= now() - ($1 || ' seconds')::interval \
     ), last_err AS ( \
        SELECT DISTINCT ON (session_id) * FROM error_candidates \
        ORDER BY session_id, created_at DESC, id DESC \
     ), eligible AS ( \
     SELECT le.session_id, s.session_name, le.id AS event_id, le.created_at AS error_at, \
            le.text, s.machine_uuid, s.adapter_id, \
            EXISTS (SELECT 1 FROM session_tokens st \
                    JOIN account_providers ap ON ap.id = st.account_id \
                    WHERE st.session_id = s.id AND ap.family = 'anthropic') AS gateway_bound, \
            EXISTS (SELECT 1 FROM stream_events p \
                    WHERE p.session_id = s.id AND p.id > r.error_event_id AND p.id < le.id \
                      AND (p.event_type = 'tool_use' OR (p.event_type = 'message' \
                           AND p.payload->>'role' = 'assistant' \
                           AND COALESCE(p.payload->>'text', '') <> '' \
                           AND p.payload->>'text' NOT LIKE 'API Error:%' \
                           AND p.payload->>'text' NOT LIKE 'Please run /login%' \
                           AND p.payload->>'text' NOT ILIKE '%automatically restarted%'))) AS made_progress, \
            r.error_event_id AS tracked_event_id, r.attempts AS tracked_attempts, \
            r.next_attempt_at AS tracked_next_at, r.state AS tracked_state \
     FROM last_err le \
     JOIN sessions s ON s.id = le.session_id \
     LEFT JOIN session_auto_resume r ON r.session_id = le.session_id \
     WHERE ",
    live_sessions_predicate!("s"),
    " AND s.machine_uuid IS NOT NULL AND s.status <> ALL($3) AND ($5::text IS NULL OR s.id = $5) \
       AND COALESCE((SELECT us.data->'autoResumeOnConnectionLoss' = 'true'::jsonb \
                     FROM user_settings us WHERE us.user_id = s.user_id), false) \
       AND NOT EXISTS (SELECT 1 FROM events ev WHERE ev.session_id = s.id \
                       AND ev.occurred_at >= le.created_at \
                       AND ev.kind IN ('session.killed', 'session.interrupted')) \
       AND NOT EXISTS ( \
           SELECT 1 FROM stream_events n \
           WHERE n.session_id = le.session_id AND n.id > le.id \
             AND (n.event_type = 'tool_use' \
                  OR (n.event_type = 'message' \
                      AND n.payload->>'role' IN ('assistant', 'user')))) \
     ) SELECT * FROM eligible WHERE ( \
       (text LIKE 'API Error:%' AND text ILIKE ANY(ARRAY[ \
           '%connection lost%', '%server error mid-response%', '%the response stopped arriving%', \
           '%stalled before a response%', '%went to sleep%'])) \
       OR (adapter_id = 'claude-code' AND gateway_bound AND machine_uuid = ANY($4) \
           AND (text LIKE 'API Error: 401 %' OR text LIKE 'Please run /login · API Error: 401 %') \
           AND (text LIKE '%Invalid bearer token%' \
                OR text LIKE '%cctui gateway rejected the session token%'))) \
       AND (tracked_state IS DISTINCT FROM 'exhausted' OR made_progress) \
     ORDER BY error_at, event_id LIMIT $2"
);

#[derive(sqlx::FromRow)]
struct StuckRow {
    session_id: String,
    session_name: Option<String>,
    event_id: i64,
    error_at: DateTime<Utc>,
    text: Option<String>,
    machine_uuid: uuid::Uuid,
    adapter_id: String,
    gateway_bound: bool,
    made_progress: bool,
    tracked_event_id: Option<i64>,
    tracked_attempts: Option<i32>,
    tracked_next_at: Option<DateTime<Utc>>,
    tracked_state: Option<String>,
}

/// A manual retry may repair a gateway-bound Claude worker even when automatic
/// recovery is disabled. The daemon rechecks its live turn and local transcript
/// before touching the worker; a cached server error alone never authorizes it.
pub async fn should_recover_gateway_auth(state: &AppState, session_id: &str) -> bool {
    let row: Option<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT s.machine_uuid, e.payload->>'text' FROM sessions s \
         JOIN LATERAL (SELECT event_type, payload FROM stream_events \
                       WHERE session_id = s.id AND (event_type = 'tool_use' OR \
                             (event_type = 'message' AND payload->>'role' IN ('assistant','user'))) \
                       ORDER BY id DESC LIMIT 1) e ON true \
         WHERE s.id = $1 AND s.machine_uuid IS NOT NULL AND s.adapter_id = 'claude-code' \
           AND s.status IN ('new','active','inactive') \
           AND e.event_type = 'message' AND e.payload->>'role' = 'assistant' \
           AND e.payload->>'text' IS NOT NULL \
           AND EXISTS (SELECT 1 FROM session_tokens st \
                       JOIN account_providers ap ON ap.id = st.account_id \
                       WHERE st.session_id=s.id AND ap.family='anthropic')"
    ).bind(session_id).fetch_optional(&state.pool).await.unwrap_or(None);
    row.is_some_and(|(machine, text)| {
        state.auth_recovery_daemons.contains_key(&machine) && is_gateway_auth_error(&text)
    })
}

/// One reaper-cadence sweep: nudge every stuck session whose backoff is due.
///
/// A session is stuck when its newest assistant message is a transport error
/// (see [`is_connection_loss`]) and no message or tool call was recorded after
/// it: the worker neither continued on its own (a cut reply that still carried
/// a complete tool call does) nor received a human reply. Archived, ended and
/// draft sessions are never touched, nor are sessions of users who did not opt
/// in. Best-effort: every failure is logged and retried on the next tick.
pub async fn sweep(state: &AppState) {
    let rows: Vec<StuckRow> = match sqlx::query_as(STUCK_SELECT)
        .bind(LOOKBACK_SECS.to_string())
        .bind(BATCH)
        .bind(SessionRowStatus::names(SessionRowStatus::NOT_RESUMABLE))
        .bind(state.auth_recovery_daemons.iter().map(|entry| *entry.key()).collect::<Vec<_>>())
        .bind(None::<&str>)
        .fetch_all(&state.pool)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("auto-resume sweep query failed: {e}");
            return;
        }
    };

    let now = Utc::now();
    for row in rows {
        let recover_auth = row.text.as_deref().is_some_and(is_gateway_auth_error);
        if recover_auth {
            if row.adapter_id != "claude-code"
                || !row.gateway_bound
                || !state.auth_recovery_daemons.contains_key(&row.machine_uuid)
            {
                continue;
            }
        } else if !row.text.as_deref().is_some_and(is_connection_loss) {
            continue;
        }
        let tracked = match (row.tracked_event_id, row.tracked_attempts, row.tracked_next_at) {
            (Some(id), Some(attempts), Some(next_at)) => {
                Some((id, attempts, next_at, row.tracked_state.as_deref() == Some("exhausted")))
            }
            _ => None,
        };
        let tracked = if row.made_progress { None } else { tracked };
        match plan(tracked, row.event_id, row.error_at, now) {
            Action::Skip => {}
            Action::Fire { attempt } => fire(state, &row, attempt, now, recover_auth).await,
            Action::Exhaust => exhaust(state, &row).await,
        }
    }
}

/// Record nudge number `attempt`, then send it. Recording first is the claim:
/// a sweep that loses the race to record the same attempt sends nothing. A
/// daemon that is away right now gets the next attempt after the backoff,
/// exactly like a nudge that reached the worker but did not wake it.
async fn fire(
    state: &AppState,
    row: &StuckRow,
    attempt: i32,
    now: DateTime<Utc>,
    recover_auth: bool,
) {
    let session_id = &row.session_id;
    // Re-read immediately before claiming: a human reply/interrupt or progress
    // may have arrived while this batch waited on another daemon.
    let current = sqlx::query_as::<_, StuckRow>(STUCK_SELECT)
        .bind(LOOKBACK_SECS.to_string())
        .bind(BATCH)
        .bind(SessionRowStatus::names(SessionRowStatus::NOT_RESUMABLE))
        .bind(state.auth_recovery_daemons.iter().map(|entry| *entry.key()).collect::<Vec<_>>())
        .bind(session_id)
        .fetch_optional(&state.pool)
        .await;
    if !current.is_ok_and(|value| value.is_some_and(|latest| latest.event_id == row.event_id)) {
        return;
    }
    let previous = row.tracked_event_id.zip(row.tracked_attempts);
    match claim_attempt(&state.pool, session_id, row.event_id, attempt, previous).await {
        Ok(true) => {}
        Ok(false) => return,
        Err(e) => {
            tracing::warn!(%session_id, "auto-resume row update failed: {e}");
            return;
        }
    }
    // Carry re-minted gateway env so a reply-driven cold-resume revives a
    // hibernated worker with a fresh token rather than empty env.
    let env = crate::routes::gateway::resume_env_for_session(state, session_id).await;
    let command_id = uuid::Uuid::new_v4();
    crate::state::track_command(
        &state.pending_commands,
        command_id,
        Some(session_id.clone()),
        None,
    );
    let dispatch = crate::bus::dispatch(
        state,
        session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: session_id.clone(),
            text: if recover_auth {
                format!("[cctui auto-resume {} attempt {attempt}/{MAX_ATTEMPTS}] The worker's gateway configuration was restored for this retry. Continue from where you left off.", now.format("%Y-%m-%dT%H:%M:%SZ"))
            } else { resume_prompt(now, attempt) },
            ask_picks: None,
            env,
            recover_auth,
            command_id: Some(command_id),
            turn_id: None,
        },
    )
    .await;
    match dispatch {
        Ok(()) => {
            tracing::info!(%session_id, attempt, "auto-resume nudge sent");
            crate::events::record(
                state,
                crate::events::Event::new(
                    crate::events::kind::SESSION_AUTO_RESUMED,
                    crate::events::Actor::Reaper,
                )
                .severity(crate::events::Severity::Warn)
                .session(session_id)
                .detail(serde_json::json!({ "attempt": attempt, "max_attempts": MAX_ATTEMPTS, "recover_auth": recover_auth })),
            );
        }
        Err(err) => {
            state.pending_commands.remove(&command_id);
            tracing::warn!(%session_id, attempt, %err, "auto-resume nudge could not be dispatched");
            let _ = sqlx::query(
                "UPDATE session_auto_resume SET last_error = $3, updated_at = now() \
                 WHERE session_id = $1 AND error_event_id = $2",
            )
            .bind(session_id)
            .bind(row.event_id)
            .bind(err.to_string())
            .execute(&state.pool)
            .await
            .map_err(|e| tracing::warn!(%session_id, "auto-resume row update failed: {e}"));
        }
    }
}

/// Write nudge `attempt` for `error_event_id`, unless this attempt (or a later
/// one) is already recorded for that error. `false` means another sweep owns it.
async fn claim_attempt(
    pool: &sqlx::PgPool,
    session_id: &str,
    error_event_id: i64,
    attempt: i32,
    previous: Option<(i64, i32)>,
) -> sqlx::Result<bool> {
    sqlx::query_scalar::<_, String>(
        "INSERT INTO session_auto_resume \
            (session_id, error_event_id, attempts, state, next_attempt_at, last_error, updated_at) \
         VALUES ($1, $2, $3, 'pending', now() + ($4 || ' seconds')::interval, NULL, now()) \
         ON CONFLICT (session_id) DO UPDATE SET \
            error_event_id = EXCLUDED.error_event_id, \
            attempts = EXCLUDED.attempts, \
            state = EXCLUDED.state, \
            next_attempt_at = EXCLUDED.next_attempt_at, \
            last_error = NULL, \
            updated_at = now() \
         WHERE session_auto_resume.error_event_id = $5 \
           AND session_auto_resume.attempts = $6 \
           AND (session_auto_resume.error_event_id <> EXCLUDED.error_event_id \
                OR session_auto_resume.attempts < EXCLUDED.attempts) \
         RETURNING session_id",
    )
    .bind(session_id)
    .bind(error_event_id)
    .bind(attempt)
    .bind(backoff_after(attempt).to_string())
    .bind(previous.map(|v| v.0))
    .bind(previous.map(|v| v.1))
    .fetch_optional(pool)
    .await
    .map(|claimed| claimed.is_some())
}

/// Mark the row exhausted and tell a human, once.
async fn exhaust(state: &AppState, row: &StuckRow) {
    let session_id = &row.session_id;
    let marked = sqlx::query(
        "UPDATE session_auto_resume SET state = 'exhausted', updated_at = now() \
         WHERE session_id = $1 AND error_event_id = $2 AND attempts = $3 AND state <> 'exhausted'",
    )
    .bind(session_id)
    .bind(row.tracked_event_id)
    .bind(row.tracked_attempts)
    .execute(&state.pool)
    .await;
    if !marked.is_ok_and(|result| result.rows_affected() == 1) {
        return;
    }
    let name = row.session_name.clone().unwrap_or_else(|| session_id.clone());
    tracing::error!(%session_id, "auto-resume gave up after {MAX_ATTEMPTS} nudges: {name}");
    crate::events::record(
        state,
        crate::events::Event::new(
            crate::events::kind::SESSION_AUTO_RESUMED,
            crate::events::Actor::Reaper,
        )
        .severity(crate::events::Severity::Error)
        .session(session_id)
        .detail(serde_json::json!({ "exhausted": true, "attempts": MAX_ATTEMPTS })),
    );
    crate::ntfy::notify(
        &state.config,
        crate::ntfy::Notification {
            title: format!("Auto-resume gave up: {name}"),
            message: format!(
                "Session {session_id} is still stuck on \"{}\" after {MAX_ATTEMPTS} automatic \
                 nudges. It needs a look.",
                row.text.as_deref().unwrap_or("API Error").trim()
            ),
            tags: "warning".into(),
            priority: 4,
        },
    );
}

#[cfg(test)]
mod tests {
    use chrono::{Duration, TimeZone, Utc};

    use super::{
        Action, MAX_ATTEMPTS, STUCK_SELECT, backoff_after, claim_attempt, first_delay_secs,
        is_connection_loss, plan,
    };

    const MIGRATION_115: &str =
        include_str!("../../../migrations/115_stream_events_api_error.up.sql");

    #[test]
    fn the_api_error_predicate_still_matches_the_partial_index() {
        for condition in [
            "event_type = 'message'",
            "payload->>'role' = 'assistant'",
            "payload->>'text' LIKE 'API Error:%'",
        ] {
            assert!(
                STUCK_SELECT.contains(&format!("e.{condition}")),
                "auto-resume query no longer contains `e.{condition}`; idx_stream_events_api_error \
                 is now orphaned — update migration 115 with it"
            );
            assert!(
                MIGRATION_115.contains(condition),
                "migration 115 no longer contains `{condition}`"
            );
        }
        assert!(
            STUCK_SELECT.contains(
                "WHERE e.event_type = 'message' AND e.payload->>'role' = 'assistant' AND \
                 e.payload->>'text' LIKE 'API Error:%'"
            ),
            "the three predicates must stay adjacent and in the index's order"
        );
    }

    #[test]
    fn recognises_every_transport_error_and_nothing_else() {
        for text in [
            "API Error: Connection lost mid-response. The response above may be incomplete.",
            "API Error: Connection lost before a response was produced. Try again.",
            "API Error: Server error mid-response. The response above may be incomplete.",
            "API Error: The response stopped arriving. The response above may be incomplete.",
            "API Error: The response stalled before a response was produced. Try again.",
            "API Error: Your computer went to sleep mid-response. The response above may be incomplete.",
            "  API Error: connection LOST mid-response.",
        ] {
            assert!(is_connection_loss(text), "{text}");
        }
        for text in [
            "API Error: 400 {\"type\":\"error\",\"error\":{\"message\":\"prompt is too long\"}}",
            "API Error: 401 authentication_error",
            "The connection was lost, let me retry.",
            "Connection lost mid-response",
            "",
        ] {
            assert!(!is_connection_loss(text), "{text}");
        }
    }

    #[test]
    fn first_nudge_waits_one_minute_after_the_error() {
        let error_at = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        assert_eq!(plan(None, 10, error_at, error_at + Duration::seconds(30)), Action::Skip);
        assert_eq!(
            plan(None, 10, error_at, error_at + Duration::seconds(60)),
            Action::Fire { attempt: 1 }
        );
    }

    #[test]
    fn tracked_row_drives_the_later_attempts_and_the_give_up() {
        let error_at = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let next = error_at + Duration::seconds(360);
        let tracked = Some((10, 1, next, false));
        assert_eq!(plan(tracked, 10, error_at, next - Duration::seconds(1)), Action::Skip);
        assert_eq!(plan(tracked, 10, error_at, next), Action::Fire { attempt: 2 });
        let all_sent = Some((10, MAX_ATTEMPTS, next, false));
        assert_eq!(plan(all_sent, 10, error_at, next), Action::Exhaust);
        let exhausted = Some((10, MAX_ATTEMPTS, next, true));
        assert_eq!(plan(exhausted, 10, error_at, next + Duration::hours(1)), Action::Skip);
    }

    #[test]
    fn a_new_error_without_success_does_not_restart_the_budget() {
        let first_error = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let second_error = first_error + Duration::minutes(20);
        let exhausted_on_first = Some((10, MAX_ATTEMPTS, first_error, true));
        assert_eq!(
            plan(exhausted_on_first, 11, second_error, second_error + Duration::seconds(10)),
            Action::Skip
        );
        assert_eq!(
            plan(exhausted_on_first, 11, second_error, second_error + Duration::seconds(60)),
            Action::Skip
        );
        assert_eq!(
            plan(None, 11, second_error, second_error + Duration::seconds(60)),
            Action::Fire { attempt: 1 }
        );
    }

    /// Doubling 1 min → 10 min, each delay within the shared ±20% jitter.
    #[test]
    fn backoff_doubles_then_caps() {
        for (attempt, base) in [(1, 120), (2, 240), (3, 480), (4, 600), (99, 600)] {
            let secs = backoff_after(attempt);
            let (lo, hi) = (base * 8 / 10, base * 12 / 10);
            assert!((lo..=hi).contains(&secs), "attempt {attempt}: {secs}s outside {lo}..={hi}");
        }
    }

    /// The first nudge is deterministic, so a sweep cannot re-roll it.
    #[test]
    fn first_delay_is_stable_and_unjittered() {
        assert_eq!(first_delay_secs(), 60);
        assert_eq!(first_delay_secs(), 60);
    }

    async fn seed_session(pool: &sqlx::PgPool, name: &str) -> String {
        let uid = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{name}-{uid}"))
            .bind(format!("h-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mk-{machine}"))
            .execute(pool)
            .await
            .unwrap();
        let sid = uuid::Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    #[tokio::test]
    async fn each_nudge_is_claimed_by_one_sweep_only() {
        let name = "auto_resume_claim";
        let Some(url) = crate::routes::gateway::test_db_url(name) else { return };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(4)
            .connect(&url)
            .await
            .expect("connect test db");
        let sid = seed_session(&pool, name).await;

        let (a, b) = tokio::join!(
            claim_attempt(&pool, &sid, 10, 1, None),
            claim_attempt(&pool, &sid, 10, 1, None)
        );
        assert_eq!(
            [a.unwrap(), b.unwrap()].iter().filter(|won| **won).count(),
            1,
            "two sweeps racing on the first nudge send it once"
        );
        assert!(!claim_attempt(&pool, &sid, 10, 1, None).await.unwrap(), "already recorded");
        assert!(
            claim_attempt(&pool, &sid, 10, 2, Some((10, 1))).await.unwrap(),
            "the next nudge is free"
        );
        assert!(!claim_attempt(&pool, &sid, 10, 1, None).await.unwrap(), "never goes backwards");
        assert!(
            claim_attempt(&pool, &sid, 11, 1, Some((10, 2))).await.unwrap(),
            "a new error restarts the budget"
        );
    }
    async fn message(pool: &sqlx::PgPool, sid: &str, role: &str, text: &str) -> i64 {
        sqlx::query_scalar("INSERT INTO stream_events (session_id, event_type, payload) VALUES ($1, 'message', $2) RETURNING id")
            .bind(sid).bind(serde_json::json!({"role":role,"text":text}))
            .fetch_one(pool).await.unwrap()
    }

    async fn selected(pool: &sqlx::PgPool, sid: &str) -> Option<super::StuckRow> {
        let machine: uuid::Uuid =
            sqlx::query_scalar("SELECT machine_uuid FROM sessions WHERE id=$1")
                .bind(sid)
                .fetch_one(pool)
                .await
                .unwrap();
        sqlx::query_as(STUCK_SELECT)
            .bind(super::LOOKBACK_SECS.to_string())
            .bind(super::BATCH)
            .bind(crate::store::sessions::SessionRowStatus::names(
                crate::store::sessions::SessionRowStatus::NOT_RESUMABLE,
            ))
            .bind(vec![machine])
            .bind(sid)
            .fetch_optional(pool)
            .await
            .unwrap()
    }

    async fn opt_in(pool: &sqlx::PgPool, sid: &str) {
        sqlx::query("INSERT INTO user_settings(user_id, data) SELECT user_id, '{\"autoResumeOnConnectionLoss\":true}'::jsonb FROM sessions WHERE id=$1")
            .bind(sid).execute(pool).await.unwrap();
    }

    #[test]
    fn login_index_matches_union_branch() {
        let migration = include_str!("../../../migrations/170_stream_events_login_error.up.sql");
        for condition in [
            "event_type = 'message'",
            "payload->>'role' = 'assistant'",
            "payload->>'text' LIKE 'Please run /login%'",
        ] {
            assert!(migration.contains(condition));
            assert!(STUCK_SELECT.contains(&format!("e.{condition}")));
        }
        assert!(STUCK_SELECT.contains("UNION ALL"));
    }

    async fn bind_gateway(pool: &sqlx::PgPool, sid: &str, uid: uuid::Uuid) {
        let account = uuid::Uuid::new_v4();
        let provider = uuid::Uuid::new_v4();
        sqlx::query("INSERT INTO accounts(id,user_id,name) VALUES($1,$2,'auth-test')")
            .bind(account)
            .bind(uid)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO account_providers(id,user_id,provider,encrypted_refresh_token,account_id) VALUES($1,$2,'anthropic','test',$3)").bind(provider).bind(uid).bind(account).execute(pool).await.unwrap();
        sqlx::query("INSERT INTO session_tokens(token_hash,session_id,account_id,revoked_at) VALUES($1,$2,$3,now())").bind(uuid::Uuid::new_v4().to_string()).bind(sid).bind(provider).execute(pool).await.unwrap();
    }

    #[tokio::test]
    async fn login_error_requires_opt_in_and_stays_bounded_until_real_progress() {
        let Some(url) = crate::routes::gateway::test_db_url("auto_resume_login") else { return };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let sid = seed_session(&pool, "login").await;
        let first = message(
            &pool,
            &sid,
            "assistant",
            "Please run /login · API Error: 401 Invalid bearer token",
        )
        .await;
        assert!(selected(&pool, &sid).await.is_none(), "default is opt-out");
        opt_in(&pool, &sid).await;
        assert!(
            selected(&pool, &sid).await.is_none(),
            "an unrelated local account is not recoverable"
        );
        // A revoked gateway credential still proves the family binding needed
        // to mint a replacement; it is never reused as a credential.
        let uid: uuid::Uuid = sqlx::query_scalar("SELECT user_id FROM sessions WHERE id=$1")
            .bind(&sid)
            .fetch_one(&pool)
            .await
            .unwrap();
        bind_gateway(&pool, &sid, uid).await;
        assert!(selected(&pool, &sid).await.unwrap().gateway_bound);
        let state = crate::state::AppState::for_test(pool.clone());
        assert!(
            !super::should_recover_gateway_auth(&state, &sid).await,
            "manual retry requires a capable daemon"
        );
        let machine: uuid::Uuid =
            sqlx::query_scalar("SELECT machine_uuid FROM sessions WHERE id=$1")
                .bind(&sid)
                .fetch_one(&pool)
                .await
                .unwrap();
        state.auth_recovery_daemons.insert(machine, ());
        sqlx::query("UPDATE user_settings SET data='{}'::jsonb WHERE user_id=$1")
            .bind(uid)
            .execute(&pool)
            .await
            .unwrap();
        assert!(
            super::should_recover_gateway_auth(&state, &sid).await,
            "an explicit retry does not need auto-resume opt-in"
        );
        sqlx::query("UPDATE user_settings SET data='{\"autoResumeOnConnectionLoss\":true}'::jsonb WHERE user_id=$1").bind(uid).execute(&pool).await.unwrap();
        let no_capability: Vec<super::StuckRow> = sqlx::query_as(STUCK_SELECT)
            .bind(super::LOOKBACK_SECS.to_string())
            .bind(super::BATCH)
            .bind(crate::store::sessions::SessionRowStatus::names(
                crate::store::sessions::SessionRowStatus::NOT_RESUMABLE,
            ))
            .bind(Vec::<uuid::Uuid>::new())
            .bind(&sid)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(no_capability.is_empty(), "old daemons must not receive a blind auth retry");
        assert!(claim_attempt(&pool, &sid, first, 1, None).await.unwrap());
        message(&pool, &sid, "user", "[cctui auto-resume attempt 1] continue").await;
        assert!(
            !super::should_recover_gateway_auth(&state, &sid).await,
            "a newer human message blocks manual stale error recovery"
        );
        assert!(selected(&pool, &sid).await.is_none(), "never replay past a later user message");
        let second = message(
            &pool,
            &sid,
            "assistant",
            "Please run /login · API Error: 401 Invalid bearer token again",
        )
        .await;
        let row = selected(&pool, &sid).await.unwrap();
        assert!(!row.made_progress, "an auto reply then another 401 is not success");
        let tracked =
            Some((row.tracked_event_id.unwrap(), row.tracked_attempts.unwrap(), Utc::now(), false));
        assert_eq!(plan(tracked, second, row.error_at, Utc::now()), Action::Fire { attempt: 2 });
        assert!(claim_attempt(&pool, &sid, second, 2, Some((first, 1))).await.unwrap());
        assert!(
            !claim_attempt(&pool, &sid, first, 1, Some((first, 1))).await.unwrap(),
            "a stale sweep cannot rewind the budget"
        );
        message(
            &pool,
            &sid,
            "assistant",
            "This session was automatically restarted after its process exited unexpectedly.",
        )
        .await;
        message(&pool, &sid, "assistant", "API Error: 401 Invalid bearer token after restart")
            .await;
        assert!(!selected(&pool, &sid).await.unwrap().made_progress);
        message(&pool, &sid, "assistant", "The build completed successfully.").await;
        message(&pool, &sid, "assistant", "API Error: 401 Invalid bearer token").await;
        assert!(selected(&pool, &sid).await.unwrap().made_progress);
    }

    #[tokio::test]
    async fn human_cancellation_new_message_and_terminal_sessions_are_not_resumed() {
        let Some(url) = crate::routes::gateway::test_db_url("auto_resume_cancel") else { return };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        for kind in ["session.killed", "session.interrupted"] {
            let sid = seed_session(&pool, kind).await;
            opt_in(&pool, &sid).await;
            message(&pool, &sid, "assistant", "API Error: Connection lost mid-response").await;
            assert!(selected(&pool, &sid).await.is_some());
            sqlx::query(
                "INSERT INTO events(session_id,kind,actor,summary) VALUES($1,$2,'user','stop')",
            )
            .bind(&sid)
            .bind(kind)
            .execute(&pool)
            .await
            .unwrap();
            assert!(selected(&pool, &sid).await.is_none());
        }
        let sid = seed_session(&pool, "later-message").await;
        opt_in(&pool, &sid).await;
        message(&pool, &sid, "assistant", "API Error: Connection lost mid-response").await;
        assert!(selected(&pool, &sid).await.is_some());
        message(&pool, &sid, "user", "Do something else").await;
        assert!(selected(&pool, &sid).await.is_none());
        for status in ["archived", "ended", "failed", "draft"] {
            let sid = seed_session(&pool, status).await;
            opt_in(&pool, &sid).await;
            message(&pool, &sid, "assistant", "API Error: Connection lost mid-response").await;
            sqlx::query("UPDATE sessions SET status=$2 WHERE id=$1")
                .bind(&sid)
                .bind(status)
                .execute(&pool)
                .await
                .unwrap();
            assert!(selected(&pool, &sid).await.is_none(), "{status}");
        }
    }
    #[tokio::test]
    async fn non_retryable_errors_do_not_consume_the_sweep_batch() {
        let Some(url) = crate::routes::gateway::test_db_url("auto_resume_batch") else { return };
        let pool = sqlx::PgPool::connect(&url).await.unwrap();
        let sid = seed_session(&pool, "batch").await;
        opt_in(&pool, &sid).await;
        for i in 0..55 {
            let other = seed_session(&pool, &format!("nonretryable-{i}")).await;
            opt_in(&pool, &other).await;
            message(&pool, &other, "assistant", "API Error: 400 prompt is too long").await;
        }
        message(&pool, &sid, "assistant", "API Error: Connection lost mid-response").await;
        let rows: Vec<super::StuckRow> = sqlx::query_as(STUCK_SELECT)
            .bind(super::LOOKBACK_SECS.to_string())
            .bind(super::BATCH)
            .bind(crate::store::sessions::SessionRowStatus::names(
                crate::store::sessions::SessionRowStatus::NOT_RESUMABLE,
            ))
            .bind(Vec::<uuid::Uuid>::new())
            .bind(None::<&str>)
            .fetch_all(&pool)
            .await
            .unwrap();
        assert!(rows.iter().any(|row| row.session_id == sid));
        assert!(rows.iter().all(|row| row.text.as_deref().is_some_and(super::is_connection_loss)));
    }
}
