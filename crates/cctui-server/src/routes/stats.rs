use std::collections::{HashMap, HashSet};

use axum::extract::{Query, State};
use axum::{Extension, Json};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;

use cctui_proto::api::{
    HeatmapCell, ModelUsage, SessionStats, TokenUsageWindows, UsageAnalytics, UsageBucket,
    WindowTokenUsage,
};
use cctui_proto::models::SessionStatus;

use crate::auth::AuthContext;
use crate::error::AppError;
use crate::live_sessions::live_sessions_predicate;
use crate::routes::sessions::{attention_from_bucket, bucket_from_signals, derive_status};
use crate::state::AppState;

/// One row of the per-session classification signals
/// (`tempo`, `agent_state`, `activity`, `soft_limit_reason`).
type SignalRow = (Option<String>, Option<String>, Option<String>, Option<String>);

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
) -> Result<Json<Vec<String>>, AppError> {
    // Scope to the caller's own sessions (admin sees all) via the
    // machine_uuid -> machines.user_id join, bound to owner_filter().
    let uid = ctx.owner_filter();
    let rows: Vec<(String,)> = match params.machine_id.as_deref() {
        Some(machine_id) => {
            sqlx::query_as(
                // normalize trailing slashes (keep root '/') so
                // `folder` and `folder/` collapse to one entry.
                "SELECT CASE WHEN s.working_dir ~ '^/+$' THEN '/' \
                    ELSE rtrim(s.working_dir, '/') END AS dir FROM sessions s \
             LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.machine_id = $1 AND s.working_dir <> '' \
             AND ($2::uuid IS NULL OR m.user_id = $2) \
             GROUP BY dir \
             ORDER BY MAX(s.registered_at) DESC LIMIT 5",
            )
            .bind(machine_id)
            .bind(uid)
            .fetch_all(&state.pool)
            .await
        }
        None => {
            sqlx::query_as(
                // normalize trailing slashes (keep root '/') so
                // `folder` and `folder/` collapse to one entry.
                "SELECT CASE WHEN s.working_dir ~ '^/+$' THEN '/' \
                    ELSE rtrim(s.working_dir, '/') END AS dir FROM sessions s \
             LEFT JOIN machines m ON m.id = s.machine_uuid \
             WHERE s.working_dir <> '' \
             AND ($1::uuid IS NULL OR m.user_id = $1) \
             GROUP BY dir \
             ORDER BY MAX(s.registered_at) DESC LIMIT 5",
            )
            .bind(uid)
            .fetch_all(&state.pool)
            .await
        }
    }?;
    Ok(Json(rows.into_iter().map(|(d,)| d).collect()))
}

/// IANA timezone from the browser; unknown or missing zones fall back to UTC.
#[derive(Debug, Default, Deserialize)]
pub struct SessionStatsParams {
    pub timezone: Option<String>,
}

// Calendar arithmetic happens before conversion to UTC, preserving DST boundaries.
// MATERIALIZED is load-bearing: inlined, the planner re-scans pg_timezone_names
// once per reference (eight times, 23-65 ms each).
const SESSION_COUNTS_SQL: &str = "
    WITH zone AS MATERIALIZED (
        SELECT COALESCE((SELECT name FROM pg_timezone_names WHERE name = $2), 'UTC') AS tz
    ), boundaries AS MATERIALIZED (
        SELECT
            date_trunc('day', $3::timestamptz AT TIME ZONE tz) AT TIME ZONE tz AS today,
            (date_trunc('day', $3::timestamptz AT TIME ZONE tz) - interval '1 day') AT TIME ZONE tz AS yesterday,
            date_trunc('week', $3::timestamptz AT TIME ZONE tz) AT TIME ZONE tz AS week,
            date_trunc('month', $3::timestamptz AT TIME ZONE tz) AT TIME ZONE tz AS month
        FROM zone
    )
    SELECT COUNT(*), COUNT(*) FILTER (WHERE s.status = 'archived'),
        COUNT(*) FILTER (WHERE s.registered_at >= b.today AND s.registered_at <= $3),
        COUNT(*) FILTER (WHERE s.registered_at >= b.yesterday AND s.registered_at < b.today),
        COUNT(*) FILTER (WHERE s.registered_at >= b.week AND s.registered_at <= $3),
        COUNT(*) FILTER (WHERE s.registered_at >= b.month AND s.registered_at <= $3)
    FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid
    CROSS JOIN boundaries b
    WHERE ($1::uuid IS NULL OR m.user_id = $1)
";

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
    Query(params): Query<SessionStatsParams>,
) -> Result<Json<SessionStats>, AppError> {
    let uid = ctx.owner_filter();

    // All counts scoped to the caller (NULL = admin sees all) via the
    // machine_uuid -> machines.user_id join.
    let (total, archived, today, yesterday, week, month): (i64, i64, i64, i64, i64, i64) =
        sqlx::query_as(SESSION_COUNTS_SQL)
            .bind(uid)
            .bind(params.timezone.as_deref().unwrap_or("UTC"))
            .bind(Utc::now())
            .fetch_one(&state.pool)
            .await?;

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
        let owned =
            crate::store::sessions::visible_session_ids(&state.pool, &live_ids, ctx.user_id)
                .await?;
        Some(owned.into_iter().collect())
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
    let signal_rows: Vec<SignalRow> = sqlx::query_as(concat!(
        "SELECT s.tempo, s.agent_state, s.activity, s.soft_limit_reason \
                 FROM sessions s LEFT JOIN machines m ON m.id = s.machine_uuid \
                 WHERE ",
        live_sessions_predicate!("s"),
        " AND ($1::uuid IS NULL OR m.user_id = $1)"
    ))
    .bind(uid)
    .fetch_all(&state.pool)
    .await?;
    let needs_input: i64 = signal_rows
        .into_iter()
        .filter(|(tempo, agent_state, activity, soft_limit_reason)| {
            attention_from_bucket(bucket_from_signals(
                tempo.as_deref(),
                agent_state.as_deref(),
                activity.as_deref(),
                soft_limit_reason.as_deref(),
                &[],
                &std::collections::HashMap::new(),
            ))
            .is_some()
        })
        .count()
        .try_into()
        .unwrap_or(i64::MAX);

    Ok(Json(SessionStats { total, live, needs_input, archived, today, yesterday, week, month }))
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
        local_now.date_naive().and_hms_opt(0, 0, 0).unwrap_or_else(|| local_now.naive_utc());
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
) -> Result<Json<TokenUsageWindows>, AppError> {
    type Row = (i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64, i64);

    let uid = ctx.owner_filter();
    let now = Utc::now();
    let hour = now - Duration::hours(1);
    let today = day_start_for_offset(now, params.tz_offset);
    let day = now - Duration::hours(24);
    let week = now - Duration::days(7);
    let month = now - Duration::days(30);

    // 15 conditional sums (5 windows × 3 metrics) in one pass. COALESCE keeps
    // every column a non-null bigint even when no rows match the window.
    // The columns read here are the INCLUDE list of
    // `idx_session_token_usage_created_covering`: reading one more drops the
    // scan off index-only and back onto ~92,000 buffers of heap.
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
    .await?;

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
const fn field(t: &TokenStatsRow, i: usize) -> i64 {
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

/// Query params for `GET /sessions/stats/usage`. `days` is the reporting range
/// (clamped 1..=365, default 30); `tz_offset` is the caller's
/// `Date.getTimezoneOffset()` (minutes; positive west of UTC), used to anchor
/// day buckets and hour-of-week extraction to the caller's local wall clock —
/// consistent with how [`session_token_stats`] anchors `today`.
#[derive(Debug, Deserialize)]
pub struct UsageAnalyticsParams {
    #[serde(default = "default_days")]
    pub days: i64,
    #[serde(default)]
    pub tz_offset: i32,
}

const fn default_days() -> i64 {
    30
}

/// Bucket granularity for a range: per-hour for short ranges (last 24-48h),
/// per-day otherwise. Kept pure for unit testing.
const fn granularity_for_days(days: i64) -> &'static str {
    if days <= 2 { "hour" } else { "day" }
}

type BucketRow = (DateTime<Utc>, i64, i64, i64, i64);
/// One [`USAGE_COST_SQL`] row: `(bucket, model, catalog, input, output, cache_read)`.
type CostRow = (DateTime<Utc>, Option<String>, Option<serde_json::Value>, i64, i64, i64);
type ModelRow = (String, i64, i64, i64, i64);
type HeatRow = (i32, i32, i64, i64);

/// Tokens-over-time aggregate. `$1` granularity (`day`/`hour`), `$2` tz-offset
/// minutes, `$3` window start, `$4` owner filter (NULL = all). Shared with the
/// aggregation test so both exercise the exact same SQL.
const USAGE_BUCKETS_SQL: &str = "SELECT \
        date_trunc($1, stu.created_at - make_interval(mins => $2)) \
            + make_interval(mins => $2) AS bucket, \
        COALESCE(SUM(stu.input_tokens), 0)::bigint, \
        COALESCE(SUM(stu.output_tokens), 0)::bigint, \
        COALESCE(SUM(stu.cache_read_tokens), 0)::bigint, \
        COALESCE(SUM(stu.cache_creation_tokens), 0)::bigint \
     FROM session_token_usage stu \
     LEFT JOIN sessions s ON s.id = stu.session_id \
     LEFT JOIN machines m ON m.id = s.machine_uuid \
     WHERE stu.created_at >= $3 AND ($4::uuid IS NULL OR m.user_id = $4) \
     GROUP BY bucket ORDER BY bucket";

/// Dollars per bucket, with the binds of [`USAGE_BUCKETS_SQL`].
///
/// Split further by model and by the catalog of the account the session last
/// drew a gateway token from, so each row can be priced the way the session
/// list prices it (`crate::cost`). A session with no token has no catalog and
/// prices to nothing.
const USAGE_COST_SQL: &str = "SELECT \
        date_trunc($1, stu.created_at - make_interval(mins => $2)) \
            + make_interval(mins => $2) AS bucket, \
        COALESCE(NULLIF(stu.model, ''), NULLIF(s.model, '')) AS model, \
        ap.models, \
        COALESCE(SUM(stu.input_tokens), 0)::bigint, \
        COALESCE(SUM(stu.output_tokens), 0)::bigint, \
        COALESCE(SUM(stu.cache_read_tokens), 0)::bigint \
     FROM session_token_usage stu \
     LEFT JOIN sessions s ON s.id = stu.session_id \
     LEFT JOIN machines m ON m.id = s.machine_uuid \
     LEFT JOIN LATERAL ( \
         SELECT st.account_id FROM session_tokens st \
         WHERE st.session_id = stu.session_id \
         ORDER BY st.created_at DESC LIMIT 1) tok ON TRUE \
     LEFT JOIN account_providers ap ON ap.id = tok.account_id \
     WHERE stu.created_at >= $3 AND ($4::uuid IS NULL OR m.user_id = $4) \
     GROUP BY 1, 2, ap.id ORDER BY 1";

/// Price every cost row and sum the dollars per bucket. A row whose model is
/// unknown to its catalog adds nothing, as everywhere else cost is derived.
fn cost_by_bucket(rows: Vec<CostRow>) -> HashMap<DateTime<Utc>, f64> {
    let mut by_bucket: HashMap<DateTime<Utc>, f64> = HashMap::new();
    for (bucket, model, catalog, input, output, cache_read) in rows {
        let cost = crate::cost::tallies_cost_usd(
            catalog.as_ref(),
            &[(model, crate::cost::TokenUsage { input, cached_input: cache_read, output })],
        );
        *by_bucket.entry(bucket).or_insert(0.0) += cost;
    }
    by_bucket
}

/// Per-model breakdown. `$1` window start, `$2` owner filter (NULL = all).
const USAGE_MODELS_SQL: &str = "SELECT \
        COALESCE(NULLIF(stu.model, ''), NULLIF(s.model, ''), 'unknown') AS model, \
        COALESCE(SUM(stu.input_tokens), 0)::bigint, \
        COALESCE(SUM(stu.output_tokens), 0)::bigint, \
        COALESCE(SUM(stu.cache_read_tokens), 0)::bigint, \
        COUNT(*)::bigint \
     FROM session_token_usage stu \
     LEFT JOIN sessions s ON s.id = stu.session_id \
     LEFT JOIN machines m ON m.id = s.machine_uuid \
     WHERE stu.created_at >= $1 AND ($2::uuid IS NULL OR m.user_id = $2) \
     GROUP BY 1 ORDER BY SUM(stu.output_tokens) DESC NULLS LAST";

/// Hour-of-week heatmap. `$1` tz-offset minutes, `$2` window start, `$3` owner
/// filter (NULL = all).
const USAGE_HEATMAP_SQL: &str = "SELECT \
        EXTRACT(dow  FROM stu.created_at - make_interval(mins => $1))::int, \
        EXTRACT(hour FROM stu.created_at - make_interval(mins => $1))::int, \
        COUNT(*)::bigint, \
        COALESCE(SUM(stu.output_tokens), 0)::bigint \
     FROM session_token_usage stu \
     LEFT JOIN sessions s ON s.id = stu.session_id \
     LEFT JOIN machines m ON m.id = s.machine_uuid \
     WHERE stu.created_at >= $2 AND ($3::uuid IS NULL OR m.user_id = $3) \
     GROUP BY 1, 2";

/// `GET /sessions/stats/usage?days=30` — Overview usage analytics:
/// tokens-over-time buckets (each carrying its dollars), per-model breakdown,
/// and an hour-of-week activity heatmap. One round-trip set (four aggregate
/// scans of `session_token_usage`, no per-bucket queries). Scoped to the
/// caller like `session_token_stats`.
///
/// Bucketing and hour-of-week extraction are done in the caller's reporting
/// timezone: `created_at` is shifted by `tz_offset` to local wall-clock time
/// before `date_trunc`/`EXTRACT`, then day buckets are mapped back to a UTC
/// instant (same convention as `today` in [`session_token_stats`]).
///
/// Model attribution prefers the per-request `session_token_usage.model` and
/// falls back to the session's own, via the PK join `sessions.id =
/// stu.session_id`; NULL/empty models bucket under `unknown`. Missing time
/// buckets are zero-filled client-side, not in SQL.
pub async fn session_usage_analytics(
    State(state): State<AppState>,
    Extension(ctx): Extension<AuthContext>,
    Query(params): Query<UsageAnalyticsParams>,
) -> Result<Json<UsageAnalytics>, AppError> {
    let uid = ctx.owner_filter();
    let days = params.days.clamp(1, 365);
    let tz = params.tz_offset;
    let granularity = granularity_for_days(days);
    let since = Utc::now() - Duration::days(days);

    let bucket_rows: Vec<BucketRow> = sqlx::query_as(USAGE_BUCKETS_SQL)
        .bind(granularity)
        .bind(tz)
        .bind(since)
        .bind(uid)
        .fetch_all(&state.pool)
        .await?;
    let cost_rows: Vec<CostRow> = sqlx::query_as(USAGE_COST_SQL)
        .bind(granularity)
        .bind(tz)
        .bind(since)
        .bind(uid)
        .fetch_all(&state.pool)
        .await?;
    let mut usd_by_bucket = cost_by_bucket(cost_rows);
    let model_rows: Vec<ModelRow> =
        sqlx::query_as(USAGE_MODELS_SQL).bind(since).bind(uid).fetch_all(&state.pool).await?;
    let heat_rows: Vec<HeatRow> = sqlx::query_as(USAGE_HEATMAP_SQL)
        .bind(tz)
        .bind(since)
        .bind(uid)
        .fetch_all(&state.pool)
        .await?;

    let cast = |v: i64| u64::try_from(v).unwrap_or(0);
    let buckets = bucket_rows
        .into_iter()
        .map(|(bucket, input, output, cache_read, cache_creation)| UsageBucket {
            cost_usd: usd_by_bucket.remove(&bucket).unwrap_or(0.0),
            bucket: bucket.to_rfc3339(),
            input: cast(input),
            output: cast(output),
            cache_read: cast(cache_read),
            cache_creation: cast(cache_creation),
        })
        .collect();
    let models = model_rows
        .into_iter()
        .map(|(model, input, output, cache_read, messages)| ModelUsage {
            model,
            input: cast(input),
            output: cast(output),
            cache_read: cast(cache_read),
            messages: cast(messages),
        })
        .collect();
    let heatmap = heat_rows
        .into_iter()
        .map(|(dow, hour, messages, output)| HeatmapCell {
            dow: dow as u8,
            hour: hour as u8,
            messages: cast(messages),
            output: cast(output),
        })
        .collect();

    Ok(Json(UsageAnalytics { granularity: granularity.into(), buckets, models, heatmap }))
}

#[cfg(test)]
mod tests {
    use super::{cost_by_bucket, day_start_for_offset, granularity_for_days};
    use chrono::{DateTime, Duration, TimeZone, Utc};
    use serde_json::json;

    fn priced_catalog() -> serde_json::Value {
        json!([{
            "model": "claude-x",
            "price_input_per_mtok": 3.0,
            "price_cached_input_per_mtok": 0.3,
            "price_output_per_mtok": 15.0
        }])
    }

    #[test]
    fn cost_by_bucket_prices_each_row_against_its_own_catalog_and_sums_per_bucket() {
        let day = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
        let other = day + Duration::days(1);
        let rows = vec![
            (day, Some("claude-x"), Some(priced_catalog()), 1_000_000, 0, 0),
            (day, Some("claude-x"), Some(priced_catalog()), 0, 100_000, 1_000_000),
            (day, Some("tokenless"), Some(priced_catalog()), 5_000_000, 5_000_000, 0),
            (day, Some("claude-x"), None, 5_000_000, 5_000_000, 0),
            (day, None, Some(priced_catalog()), 5_000_000, 5_000_000, 0),
            (other, Some("claude-x"), Some(priced_catalog()), 0, 1_000_000, 0),
        ]
        .into_iter()
        .map(|(bucket, model, catalog, input, output, cache_read)| {
            (bucket, model.map(str::to_owned), catalog, input, output, cache_read)
        })
        .collect();
        let by = cost_by_bucket(rows);
        assert_eq!(by.len(), 2);
        assert!((by[&day] - (3.0 + 1.5 + 0.3)).abs() < 1e-9, "{by:?}");
        assert!((by[&other] - 15.0).abs() < 1e-9, "{by:?}");
    }

    #[test]
    fn cost_by_bucket_is_empty_without_rows() {
        assert!(cost_by_bucket(Vec::new()).is_empty());
    }

    /// A user with one priced anthropic credential and two sessions that each
    /// burned the same tokens two days ago: only `priced` holds a token bound
    /// to that credential. Returns the user and the two session ids.
    async fn seed_priced_and_tokenless_sessions(
        pool: &sqlx::PgPool,
        at: DateTime<Utc>,
    ) -> (uuid::Uuid, String, String) {
        use uuid::Uuid;

        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        let priced = format!("cost-agg-{uid}-priced");
        let tokenless = format!("cost-agg-{uid}-tokenless");
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("cost-agg-{uid}"))
            .bind(format!("hcost-{uid}"))
            .execute(pool)
            .await
            .expect("insert user");
        sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
            .bind(machine)
            .bind(uid)
            .bind(format!("mcost-{machine}"))
            .execute(pool)
            .await
            .expect("insert machine");
        let account: Uuid =
            sqlx::query_scalar("INSERT INTO accounts (user_id, name) VALUES ($1, $2) RETURNING id")
                .bind(uid)
                .bind(format!("cost-account-{uid}"))
                .fetch_one(pool)
                .await
                .expect("insert account");
        let provider: Uuid = sqlx::query_scalar(
            "INSERT INTO account_providers (user_id, account_id, provider, models) \
             VALUES ($1, $2, 'anthropic', $3) RETURNING id",
        )
        .bind(uid)
        .bind(account)
        .bind(priced_catalog())
        .fetch_one(pool)
        .await
        .expect("insert provider");
        for sid in [&priced, &tokenless] {
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, model) \
                 VALUES ($1, $2, $2, $3, '/w', 'archived', 'claude-x')",
            )
            .bind(sid)
            .bind(machine)
            .bind(uid)
            .execute(pool)
            .await
            .expect("insert session");
            sqlx::query(
                "INSERT INTO session_token_usage (session_id, message_id, model, input_tokens, \
                 output_tokens, cache_read_tokens, cache_creation_tokens, created_at) \
                 VALUES ($1, $2, 'claude-x', 1000000, 100000, 1000000, 0, $3)",
            )
            .bind(sid)
            .bind(format!("{sid}-m1"))
            .bind(at)
            .execute(pool)
            .await
            .expect("insert usage");
        }
        sqlx::query(
            "INSERT INTO session_tokens (token_hash, session_id, account_id) VALUES ($1, $2, $3)",
        )
        .bind(format!("hash-{}", Uuid::new_v4()))
        .bind(&priced)
        .bind(provider)
        .execute(pool)
        .await
        .expect("insert token");
        (uid, priced, tokenless)
    }

    async fn remove_seeded(pool: &sqlx::PgPool, uid: uuid::Uuid, sessions: &[&str]) {
        for sid in sessions {
            sqlx::query("DELETE FROM session_tokens WHERE session_id = $1")
                .bind(sid)
                .execute(pool)
                .await
                .expect("cleanup tokens");
            sqlx::query("DELETE FROM sessions WHERE id = $1")
                .bind(sid)
                .execute(pool)
                .await
                .expect("cleanup session");
        }
        sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(uid)
            .execute(pool)
            .await
            .expect("cleanup user");
    }

    #[tokio::test]
    async fn usage_cost_over_db() {
        use super::{CostRow, USAGE_COST_SQL};

        let Some(url) = crate::routes::gateway::test_db_url("usage_cost_over_db") else {
            return;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db");

        let midday = Utc::now().date_naive().and_hms_opt(12, 0, 0).expect("midday").and_utc()
            - Duration::days(2);
        let (uid, priced, tokenless) = seed_priced_and_tokenless_sessions(&pool, midday).await;

        let rows: Vec<CostRow> = sqlx::query_as(USAGE_COST_SQL)
            .bind("day")
            .bind(0_i32)
            .bind(Utc::now() - Duration::days(30))
            .bind(Some(uid))
            .fetch_all(&pool)
            .await
            .expect("cost query");
        let got = cost_by_bucket(rows)
            .iter()
            .find(|(b, _)| b.date_naive() == midday.date_naive())
            .map_or(0.0, |(_, v)| *v);
        assert!(
            (got - 4.8).abs() < 1e-9,
            "only the session with a priced catalog counts, got {got}"
        );

        remove_seeded(&pool, uid, &[&priced, &tokenless]).await;
    }

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
