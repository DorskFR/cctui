//! Auto-resume after a mid-stream connection loss (opt-in per user via the
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
/// A different `error_event_id` means a new occurrence after a successful
/// resume: the budget starts again from the error's own timestamp.
#[must_use]
pub fn plan(
    tracked: Option<(i64, i32, DateTime<Utc>, bool)>,
    error_event_id: i64,
    error_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Action {
    let (attempts, due) = match tracked {
        Some((id, attempts, next_attempt_at, exhausted)) if id == error_event_id => {
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
    "WITH last_err AS ( \
        SELECT DISTINCT ON (e.session_id) \
               e.session_id, e.id, e.created_at, e.payload->>'text' AS text \
        FROM stream_events e \
        WHERE e.event_type = 'message' \
          AND e.payload->>'role' = 'assistant' \
          AND e.payload->>'text' LIKE 'API Error:%' \
          AND e.created_at >= now() - ($1 || ' seconds')::interval \
        ORDER BY e.session_id, e.created_at DESC, e.id DESC \
     ) \
     SELECT le.session_id, s.session_name, le.id AS event_id, le.created_at AS error_at, \
            le.text, \
            r.error_event_id AS tracked_event_id, r.attempts AS tracked_attempts, \
            r.next_attempt_at AS tracked_next_at, r.state AS tracked_state \
     FROM last_err le \
     JOIN sessions s ON s.id = le.session_id \
     LEFT JOIN session_auto_resume r ON r.session_id = le.session_id \
     WHERE ",
    live_sessions_predicate!("s"),
    " AND s.status <> ALL($3) \
       AND COALESCE((SELECT us.data->'autoResumeOnConnectionLoss' = 'true'::jsonb \
                     FROM user_settings us WHERE us.user_id = s.user_id), false) \
       AND NOT EXISTS ( \
           SELECT 1 FROM stream_events n \
           WHERE n.session_id = le.session_id AND n.id > le.id \
             AND (n.event_type = 'tool_use' \
                  OR (n.event_type = 'message' \
                      AND n.payload->>'role' IN ('assistant', 'user')))) \
     LIMIT $2"
);

#[derive(sqlx::FromRow)]
struct StuckRow {
    session_id: String,
    session_name: Option<String>,
    event_id: i64,
    error_at: DateTime<Utc>,
    text: Option<String>,
    tracked_event_id: Option<i64>,
    tracked_attempts: Option<i32>,
    tracked_next_at: Option<DateTime<Utc>>,
    tracked_state: Option<String>,
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
        if !row.text.as_deref().is_some_and(is_connection_loss) {
            continue;
        }
        let tracked = match (row.tracked_event_id, row.tracked_attempts, row.tracked_next_at) {
            (Some(id), Some(attempts), Some(next_at)) => {
                Some((id, attempts, next_at, row.tracked_state.as_deref() == Some("exhausted")))
            }
            _ => None,
        };
        match plan(tracked, row.event_id, row.error_at, now) {
            Action::Skip => {}
            Action::Fire { attempt } => fire(state, &row, attempt, now).await,
            Action::Exhaust => exhaust(state, &row).await,
        }
    }
}

/// Record nudge number `attempt`, then send it. Recording first is the claim:
/// a sweep that loses the race to record the same attempt sends nothing. A
/// daemon that is away right now gets the next attempt after the backoff,
/// exactly like a nudge that reached the worker but did not wake it.
async fn fire(state: &AppState, row: &StuckRow, attempt: i32, now: DateTime<Utc>) {
    let session_id = &row.session_id;
    match claim_attempt(&state.pool, session_id, row.event_id, attempt).await {
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
    let dispatch = crate::bus::dispatch(
        state,
        session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: session_id.clone(),
            text: resume_prompt(now, attempt),
            ask_picks: None,
            env,
            command_id: None,
            turn_id: None,
        },
    )
    .await;
    match dispatch {
        Ok(()) => {
            tracing::info!(%session_id, attempt, "auto-resume nudge sent after connection loss");
        }
        Err(err) => {
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
         WHERE session_auto_resume.error_event_id <> EXCLUDED.error_event_id \
            OR session_auto_resume.attempts < EXCLUDED.attempts \
         RETURNING session_id",
    )
    .bind(session_id)
    .bind(error_event_id)
    .bind(attempt)
    .bind(backoff_after(attempt).to_string())
    .fetch_optional(pool)
    .await
    .map(|claimed| claimed.is_some())
}

/// Mark the row exhausted and tell a human, once.
async fn exhaust(state: &AppState, row: &StuckRow) {
    let session_id = &row.session_id;
    let _ = sqlx::query(
        "UPDATE session_auto_resume SET state = 'exhausted', updated_at = now() \
         WHERE session_id = $1 AND error_event_id = $2",
    )
    .bind(session_id)
    .bind(row.event_id)
    .execute(&state.pool)
    .await
    .map_err(|e| tracing::warn!(%session_id, "auto-resume row update failed: {e}"));
    let name = row.session_name.clone().unwrap_or_else(|| session_id.clone());
    tracing::error!(%session_id, "auto-resume gave up after {MAX_ATTEMPTS} nudges: {name}");
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
    fn a_new_error_after_a_successful_resume_restarts_the_budget() {
        let first_error = Utc.with_ymd_and_hms(2026, 9, 4, 12, 0, 0).unwrap();
        let second_error = first_error + Duration::minutes(20);
        let exhausted_on_first = Some((10, MAX_ATTEMPTS, first_error, true));
        assert_eq!(
            plan(exhausted_on_first, 11, second_error, second_error + Duration::seconds(10)),
            Action::Skip
        );
        assert_eq!(
            plan(exhausted_on_first, 11, second_error, second_error + Duration::seconds(60)),
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

        let (a, b) =
            tokio::join!(claim_attempt(&pool, &sid, 10, 1), claim_attempt(&pool, &sid, 10, 1));
        assert_eq!(
            [a.unwrap(), b.unwrap()].iter().filter(|won| **won).count(),
            1,
            "two sweeps racing on the first nudge send it once"
        );
        assert!(!claim_attempt(&pool, &sid, 10, 1).await.unwrap(), "already recorded");
        assert!(claim_attempt(&pool, &sid, 10, 2).await.unwrap(), "the next nudge is free");
        assert!(!claim_attempt(&pool, &sid, 10, 1).await.unwrap(), "never goes backwards");
        assert!(
            claim_attempt(&pool, &sid, 11, 1).await.unwrap(),
            "a new error restarts the budget"
        );
    }
}
