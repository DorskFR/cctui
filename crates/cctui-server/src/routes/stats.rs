use std::collections::HashSet;

use axum::Extension;
use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use cctui_proto::api::{ApiError, SessionStats, TokenUsageWindows, WindowTokenUsage};
use cctui_proto::models::SessionStatus;

use crate::auth::AuthContext;
use crate::routes::sessions::{attention_from_bucket, bucket_from_signals, derive_status};
use crate::state::AppState;

/// Query params for `GET /sessions/recent-dirs` — the last working dirs used
/// on a given machine, for the spawn working-directory picker.
#[derive(Debug, Default, Deserialize)]
pub struct RecentDirsParams {
    pub machine_id: Option<String>,
}

/// `GET /sessions/recent-dirs?machine_id=…` → up to 5 distinct working dirs
/// most recently used on that machine (most-recent first). With no
/// `machine_id`, returns the most recent dirs across all machines.
pub async fn recent_dirs(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<RecentDirsParams>,
) -> Result<Json<Vec<String>>, (StatusCode, Json<ApiError>)> {
    // Scope to the caller's own sessions (admin sees all) via the
    // machine_uuid -> machines.user_id join, bound to owner_filter().
    let uid = ctx.owner_filter();
    let rows: Vec<(String,)> = match params.machine_id.as_deref() {
        Some(machine_id) => {
            sqlx::query_as(
                "SELECT s.working_dir FROM sessions s \
             LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.machine_id = $1 AND s.working_dir <> '' \
             AND ($2::uuid IS NULL OR m.user_id = $2) \
             GROUP BY s.working_dir \
             ORDER BY MAX(s.registered_at) DESC LIMIT 5",
            )
            .bind(machine_id)
            .bind(uid)
            .fetch_all(&state.pool)
            .await
        }
        None => {
            sqlx::query_as(
                "SELECT s.working_dir FROM sessions s \
             LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.working_dir <> '' \
             AND ($1::uuid IS NULL OR m.user_id = $1) \
             GROUP BY s.working_dir \
             ORDER BY MAX(s.registered_at) DESC LIMIT 5",
            )
            .bind(uid)
            .fetch_all(&state.pool)
            .await
        }
    }
    .map_err(|e| {
        tracing::error!("db error (recent dirs): {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    Ok(Json(rows.into_iter().map(|(d,)| d).collect()))
}

/// `GET /sessions/stats` — aggregate session counts for the Overview page.
///
/// The session list is capped (`LIMIT 25`), so counting client-side over it
/// undercounts once there are more than 25 sessions. This computes the totals
/// straight from SQL aggregates (`total`, `archived`) and the live registry
/// (`live`), and counts `needs_input` by running the classifier over every
/// non-archived session's persisted signals.
pub async fn session_stats(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
) -> Result<Json<SessionStats>, (StatusCode, Json<ApiError>)> {
    let db_err = |e: sqlx::Error| {
        tracing::error!("db error (session stats): {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    };
    let uid = ctx.owner_filter();

    // All counts scoped to the caller (NULL = admin sees all) via the
    // machine_uuid -> machines.user_id join.
    let (total, archived): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE s.status = 'archived') \
         FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE ($1::uuid IS NULL OR m.user_id = $1)",
    )
    .bind(uid)
    .fetch_one(&state.pool)
    .await
    .map_err(db_err)?;

    // Live = sessions currently in the registry whose derived status is
    // active/new (matches how the list surfaces "live"). Scope to the caller's
    // owned live ids for non-admins (resolved from the DB, like list_sessions).
    let owned_live_ids: Option<HashSet<String>> = if ctx.is_admin() {
        None
    } else {
        let live_ids: Vec<String> = {
            let registry = state.registry.read().await;
            registry.list().into_iter().map(|h| h.session.id.clone()).collect()
        };
        let rows: Vec<(String,)> = sqlx::query_as(
            "SELECT s.id FROM sessions s \
             LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.id = ANY($1) AND m.user_id = $2",
        )
        .bind(&live_ids)
        .bind(ctx.user_id)
        .fetch_all(&state.pool)
        .await
        .map_err(db_err)?;
        Some(rows.into_iter().map(|(id,)| id).collect())
    };
    let live: i64 = {
        let registry = state.registry.read().await;
        registry
            .list()
            .into_iter()
            .filter(|h| owned_live_ids.as_ref().is_none_or(|owned| owned.contains(&h.session.id)))
            .filter(|h| {
                matches!(
                    derive_status(h.session.registered_at, h.session.last_heartbeat),
                    SessionStatus::Active | SessionStatus::New
                )
            })
            .count()
            .try_into()
            .unwrap_or(i64::MAX)
    };

    // needs_input: classify every non-archived session from its persisted
    // signals and count the Blocked bucket — scoped to the caller.
    let signal_rows: Vec<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT s.tempo, s.agent_state, s.activity \
         FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.status != 'archived' AND ($1::uuid IS NULL OR m.user_id = $1)",
    )
    .bind(uid)
    .fetch_all(&state.pool)
    .await
    .map_err(db_err)?;
    let needs_input: i64 = signal_rows
        .into_iter()
        .filter(|(tempo, agent_state, activity)| {
            attention_from_bucket(bucket_from_signals(
                tempo.as_deref(),
                agent_state.as_deref(),
                activity.as_deref(),
            ))
            .is_some()
        })
        .count()
        .try_into()
        .unwrap_or(i64::MAX);

    Ok(Json(SessionStats { total, live, needs_input, archived }))
}

/// Query params for `GET /sessions/stats/tokens`. `tz_offset` is the caller's
/// `Date.getTimezoneOffset()` (minutes; positive west of UTC), used only to
/// anchor the calendar "today" window to local midnight. Defaults to UTC.
#[derive(Debug, Default, Deserialize)]
pub struct TokenStatsParams {
    #[serde(default)]
    pub tz_offset: i32,
}

/// UTC instant of the most recent local midnight, given the caller's
/// `Date.getTimezoneOffset()` value. JS reports local = UTC − offset, so the
/// UTC instant of a local wall-clock time L is `L + offset`.
fn day_start_for_offset(now: DateTime<Utc>, tz_offset_minutes: i32) -> DateTime<Utc> {
    let offset = Duration::minutes(i64::from(tz_offset_minutes));
    let local_now = now - offset;
    // Truncate the local wall-clock time to midnight, then map back to UTC.
    let local_midnight =
        local_now.date_naive().and_hms_opt(0, 0, 0).unwrap_or(local_now.naive_utc());
    DateTime::<Utc>::from_naive_utc_and_offset(local_midnight, Utc) + offset
}

/// `GET /sessions/stats/tokens` — token totals across rolling time windows for
/// the Overview. One scan of `session_token_usage` with per-window conditional
/// aggregates. Each window reports the same three figures the session card
/// shows (`↑input ↓output ⚡cache_read`). Global, like `session_stats`.
pub async fn session_token_stats(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<TokenStatsParams>,
) -> Result<Json<TokenUsageWindows>, (StatusCode, Json<ApiError>)> {
    let uid = ctx.owner_filter();
    let now = Utc::now();
    let hour = now - Duration::hours(1);
    let today = day_start_for_offset(now, params.tz_offset);
    let day = now - Duration::hours(24);
    let week = now - Duration::days(7);
    let month = now - Duration::days(30);

    // 15 conditional sums (5 windows × 3 metrics) in one pass. COALESCE keeps
    // every column a non-null bigint even when no rows match the window.
    type Row = (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64);
    let r: Row = sqlx::query_as(
        "SELECT \
            COALESCE(SUM(input_tokens)       FILTER (WHERE created_at >= $1), 0)::bigint, \
            COALESCE(SUM(output_tokens)      FILTER (WHERE created_at >= $1), 0)::bigint, \
            COALESCE(SUM(cache_read_tokens)  FILTER (WHERE created_at >= $1), 0)::bigint, \
            COALESCE(SUM(input_tokens)       FILTER (WHERE created_at >= $2), 0)::bigint, \
            COALESCE(SUM(output_tokens)      FILTER (WHERE created_at >= $2), 0)::bigint, \
            COALESCE(SUM(cache_read_tokens)  FILTER (WHERE created_at >= $2), 0)::bigint, \
            COALESCE(SUM(input_tokens)       FILTER (WHERE created_at >= $3), 0)::bigint, \
            COALESCE(SUM(output_tokens)      FILTER (WHERE created_at >= $3), 0)::bigint, \
            COALESCE(SUM(cache_read_tokens)  FILTER (WHERE created_at >= $3), 0)::bigint, \
            COALESCE(SUM(input_tokens)       FILTER (WHERE created_at >= $4), 0)::bigint, \
            COALESCE(SUM(output_tokens)      FILTER (WHERE created_at >= $4), 0)::bigint, \
            COALESCE(SUM(cache_read_tokens)  FILTER (WHERE created_at >= $4), 0)::bigint, \
            COALESCE(SUM(input_tokens)       FILTER (WHERE created_at >= $5), 0)::bigint, \
            COALESCE(SUM(output_tokens)      FILTER (WHERE created_at >= $5), 0)::bigint, \
            COALESCE(SUM(cache_read_tokens)  FILTER (WHERE created_at >= $5), 0)::bigint \
         FROM session_token_usage stu \
         LEFT JOIN sessions s ON s.id = stu.session_id \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE stu.created_at >= $5 AND ($6::uuid IS NULL OR m.user_id = $6)",
    )
    .bind(hour)
    .bind(today)
    .bind(day)
    .bind(week)
    .bind(month)
    .bind(uid)
    .fetch_one(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error (token stats): {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    let cast = |v: i64| u64::try_from(v).unwrap_or(0);
    let win = |i: usize, o: usize, c: usize, t: &Row| WindowTokenUsage {
        input: cast(field(t, i)),
        output: cast(field(t, o)),
        cache_read: cast(field(t, c)),
    };
    Ok(Json(TokenUsageWindows {
        hour: win(0, 1, 2, &r),
        today: win(3, 4, 5, &r),
        day: win(6, 7, 8, &r),
        week: win(9, 10, 11, &r),
        month: win(12, 13, 14, &r),
    }))
}

/// The 15-column token-stats row (the `query_as` row above), indexed
/// positionally by [`field`] to keep the window construction terse without a
/// 15-field named struct.
type TokenStatsRow = (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64);

/// Index a 15-tuple positionally (the `query_as` row above). Keeps the window
/// construction terse without a 15-field named struct.
fn field(t: &TokenStatsRow, i: usize) -> i64 {
    match i {
        0 => t.0,
        1 => t.1,
        2 => t.2,
        3 => t.3,
        4 => t.4,
        5 => t.5,
        6 => t.6,
        7 => t.7,
        8 => t.8,
        9 => t.9,
        10 => t.10,
        11 => t.11,
        12 => t.12,
        13 => t.13,
        _ => t.14,
    }
}

#[cfg(test)]
mod tests {
    use super::day_start_for_offset;
    use chrono::{DateTime, TimeZone, Utc};

    #[test]
    fn day_start_utc_when_no_offset() {
        // 2026-06-11T09:30Z with offset 0 → midnight the same UTC day.
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 9, 30, 0).unwrap();
        let start = day_start_for_offset(now, 0);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 6, 11, 0, 0, 0).unwrap());
    }

    #[test]
    fn day_start_uses_local_midnight_west_of_utc() {
        // tz_offset +480 (UTC−8). At 2026-06-11T05:00Z it's still 2026-06-10
        // 21:00 locally, so "today" started at 2026-06-10T08:00Z (local midnight).
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 5, 0, 0).unwrap();
        let start = day_start_for_offset(now, 480);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 6, 10, 8, 0, 0).unwrap());
    }

    #[test]
    fn day_start_uses_local_midnight_east_of_utc() {
        // tz_offset −120 (UTC+2, Europe/Paris summer). At 2026-06-11T01:00Z it's
        // already 03:00 on the 11th locally, so local midnight was 2026-06-10T22:00Z.
        let now = Utc.with_ymd_and_hms(2026, 6, 11, 1, 0, 0).unwrap();
        let start = day_start_for_offset(now, -120);
        assert_eq!(start, Utc.with_ymd_and_hms(2026, 6, 10, 22, 0, 0).unwrap());
        // Sanity: the boundary is in the past relative to `now`.
        let _: DateTime<Utc> = start;
        assert!(start < now);
    }
}
