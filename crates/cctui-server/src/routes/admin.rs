use std::collections::HashSet;

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde::Deserialize;

use cctui_proto::api::{
    ApiError, MessageRequest, RenameRequest, SessionListItem, SessionListResponse, SessionStats,
};
use cctui_proto::classifier::{Bucket, ClassifyInput, PrStatus, classify};
use cctui_proto::models::{Attention, Liveness, SessionStatus};

use crate::state::AppState;

/// Derived-status thresholds. A session is considered:
/// - `Active` if its last heartbeat is within this window;
/// - `New` if it isn't active but was registered within this window;
/// - `Inactive` otherwise.
const STATUS_WINDOW_SECS: i64 = 5 * 60;

/// Liveness-dot thresholds (heartbeat age). `Active` (green) within the
/// active window, `Stale` (orange) up to the dead window, `Dead` (no dot)
/// beyond it.
const LIVENESS_ACTIVE_SECS: i64 = 5 * 60;
const LIVENESS_DEAD_SECS: i64 = 60 * 60;

/// Query params for `GET /sessions`. Archived sessions are hidden unless
/// `?include_archived=true`.
#[derive(Debug, Default, Deserialize)]
pub struct ListParams {
    #[serde(default)]
    pub include_archived: bool,
}

#[derive(sqlx::FromRow)]
struct DbSession {
    id: String,
    parent_id: Option<String>,
    machine_id: String,
    working_dir: String,
    status: String,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
    metadata: serde_json::Value,
    adapter_id: Option<String>,
    resolved_machine_name: Option<String>,
    resolved_machine_hue: Option<i16>,
    resolved_machine_kind: Option<String>,
}

fn derive_status(registered_at: DateTime<Utc>, last_heartbeat: DateTime<Utc>) -> SessionStatus {
    let now = Utc::now();
    if (now - last_heartbeat).num_seconds() < STATUS_WINDOW_SECS {
        SessionStatus::Active
    } else if (now - registered_at).num_seconds() < STATUS_WINDOW_SECS {
        SessionStatus::New
    } else {
        SessionStatus::Inactive
    }
}

/// Map heartbeat age onto the three-tier liveness dot.
fn derive_liveness(last_heartbeat: DateTime<Utc>) -> Liveness {
    let age = (Utc::now() - last_heartbeat).num_seconds();
    if age < LIVENESS_ACTIVE_SECS {
        Liveness::Active
    } else if age < LIVENESS_DEAD_SECS {
        Liveness::Stale
    } else {
        Liveness::Dead
    }
}

/// Sticky terminal statuses (CCT-192): persisted states that must NOT be
/// re-derived from heartbeat age. `ended` (`SessionEnded` received) and `failed`
/// (dispatch never launched) both mean "this session is over" — without this
/// they showed Active/green for ~5 min until the heartbeat aged out, masking
/// the end of unattended/dispatched jobs.
fn sticky_status(row_status: &str) -> Option<SessionStatus> {
    match row_status {
        "archived" => Some(SessionStatus::Archived),
        "ended" | "failed" => Some(SessionStatus::Inactive),
        _ => None,
    }
}

/// Resolve the row's status + liveness, honouring sticky terminal states.
fn resolve_status_liveness(
    row_status: &str,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
) -> (SessionStatus, Liveness) {
    sticky_status(row_status).map_or_else(
        || (derive_status(registered_at, last_heartbeat), derive_liveness(last_heartbeat)),
        |s| {
            // Archived keeps its real liveness dot; ended/failed are terminal → Dead.
            let liveness = if matches!(s, SessionStatus::Archived) {
                derive_liveness(last_heartbeat)
            } else {
                Liveness::Dead
            };
            (s, liveness)
        },
    )
}

/// Classify a session into its bucket from the persisted signals. PR-children
/// ("Ready for review") are not wired server-side yet, so `children` is empty
/// and the PR cache unused — the `Review` bucket therefore cannot arise today.
fn bucket_from_signals(
    tempo: Option<&str>,
    agent_state: Option<&str>,
    activity: Option<&str>,
) -> Bucket {
    let input = ClassifyInput { tempo, state: agent_state, activity, children: &[], q: None };
    let empty: std::collections::HashMap<String, PrStatus<'_>> = std::collections::HashMap::new();
    classify(&input, &empty)
}

/// The attention glyph is just the `Blocked` bucket surfaced as ✋ needs input.
const fn attention_from_bucket(bucket: Bucket) -> Option<Attention> {
    match bucket {
        Bucket::Blocked => Some(Attention::NeedsInput),
        _ => None,
    }
}

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
    Query(params): Query<RecentDirsParams>,
) -> Result<Json<Vec<String>>, (StatusCode, Json<ApiError>)> {
    let rows: Vec<(String,)> = match params.machine_id.as_deref() {
        Some(machine_id) => {
            sqlx::query_as(
                "SELECT working_dir FROM sessions \
             WHERE machine_id = $1 AND working_dir <> '' \
             GROUP BY working_dir \
             ORDER BY MAX(registered_at) DESC LIMIT 5",
            )
            .bind(machine_id)
            .fetch_all(&state.pool)
            .await
        }
        None => {
            sqlx::query_as(
                "SELECT working_dir FROM sessions \
             WHERE working_dir <> '' \
             GROUP BY working_dir \
             ORDER BY MAX(registered_at) DESC LIMIT 5",
            )
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

#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
pub async fn list_sessions(
    State(state): State<AppState>,
    Query(params): Query<ListParams>,
) -> Result<Json<SessionListResponse>, (StatusCode, Json<ApiError>)> {
    // Live sessions from in-memory registry — keep registered_at for sorting.
    let mut with_ts: Vec<(DateTime<Utc>, SessionListItem)> = {
        let registry = state.registry.read().await;
        registry
            .list()
            .into_iter()
            .map(|handle| {
                (
                    handle.session.registered_at,
                    SessionListItem {
                        id: handle.session.id.clone(),
                        parent_id: handle.session.parent_id.clone(),
                        machine_id: handle.session.machine_id.clone(),
                        working_dir: handle.session.working_dir.clone(),
                        status: derive_status(
                            handle.session.registered_at,
                            handle.session.last_heartbeat,
                        ),
                        liveness: derive_liveness(handle.session.last_heartbeat),
                        attention: None,
                        bucket: Bucket::Working,
                        uptime_secs: (Utc::now() - handle.session.registered_at).num_seconds(),
                        token_usage: handle.token_usage.clone(),
                        metadata: handle.session.metadata.clone(),
                        adapter_id: handle.session.adapter_id.clone(),
                        machine_name: None,
                        machine_hue: None,
                        machine_kind: None,
                        last_message_text: None,
                        last_message_at: None,
                        name: None,
                        model: None,
                        effort: None,
                        auto_approve: false,
                        match_snippet: None,
                        last_activity_at: None,
                        cache_cold: false,
                        estimated_burst_tokens: None,
                        hibernated: false,
                    },
                )
            })
            .collect()
    };

    // Historical inactive sessions from DB (not currently in the live registry).
    // Archived sessions are hidden unless explicitly requested.
    let live_ids: HashSet<String> = with_ts.iter().map(|(_, s)| s.id.clone()).collect();
    let archived_filter = if params.include_archived { "" } else { "WHERE s.status != 'archived'" };
    let query = format!(
        "SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.status, \
                s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, \
                COALESCE(m.display_name, m.name) AS resolved_machine_name, \
                m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind \
         FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         {archived_filter} \
         ORDER BY s.registered_at DESC LIMIT 25",
    );
    let rows: Vec<DbSession> =
        sqlx::query_as(&query).fetch_all(&state.pool).await.map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;

    for row in rows {
        if live_ids.contains(&row.id) {
            continue;
        }
        // Sticky terminal states (archived/ended/failed) are NOT re-derived
        // from heartbeat; everything else is time-based.
        let (status, liveness) =
            resolve_status_liveness(&row.status, row.registered_at, row.last_heartbeat);
        with_ts.push((
            row.registered_at,
            SessionListItem {
                id: row.id,
                parent_id: row.parent_id,
                machine_id: row.machine_id,
                working_dir: row.working_dir,
                status,
                liveness,
                attention: None,
                bucket: Bucket::Working,
                uptime_secs: (Utc::now() - row.registered_at).num_seconds(),
                token_usage: cctui_proto::models::TokenUsage::default(),
                metadata: row.metadata,
                adapter_id: row.adapter_id.map(cctui_proto::adapter::AdapterId::new),
                machine_name: row.resolved_machine_name,
                machine_hue: row.resolved_machine_hue,
                machine_kind: row.resolved_machine_kind,
                last_message_text: None,
                last_message_at: None,
                name: None,
                model: None,
                effort: None,
                auto_approve: false,
                match_snippet: None,
                last_activity_at: None,
                cache_cold: false,
                estimated_burst_tokens: None,
                hibernated: false,
            },
        ));
    }

    let sessions = enrich_and_sort(&state, with_ts).await?;
    Ok(Json(SessionListResponse { sessions }))
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
) -> Result<Json<SessionStats>, (StatusCode, Json<ApiError>)> {
    let db_err = |e: sqlx::Error| {
        tracing::error!("db error (session stats): {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    };

    let (total, archived): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COUNT(*) FILTER (WHERE status = 'archived') FROM sessions",
    )
    .fetch_one(&state.pool)
    .await
    .map_err(db_err)?;

    // Live = sessions currently in the registry whose derived status is
    // active/new (matches how the list surfaces "live").
    let live: i64 = {
        let registry = state.registry.read().await;
        registry
            .list()
            .into_iter()
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
    // signals and count the Blocked bucket.
    let signal_rows: Vec<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT tempo, agent_state, activity FROM sessions WHERE status != 'archived'",
    )
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

/// Shared enrichment for both the sessions list and search: resolve machine
/// names, aggregate token usage, attach last-message text, apply classifier
/// signals + display metadata + the in-memory auto-approve flag, then sort
/// most-recent-first. Returns the finished items ready to serialize.
#[allow(clippy::too_many_lines, clippy::cognitive_complexity)]
async fn enrich_and_sort(
    state: &AppState,
    mut with_ts: Vec<(DateTime<Utc>, SessionListItem)>,
) -> Result<Vec<SessionListItem>, (StatusCode, Json<ApiError>)> {
    // Resolve machine names in one query. Historical sessions for purged
    // machines simply get `None`.
    let machine_ids: Vec<String> = with_ts
        .iter()
        .map(|(_, s)| s.machine_id.clone())
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    if !machine_ids.is_empty() {
        // `sessions.machine_id` is freeform: daemon-spawned sessions carry the
        // machine UUID, but legacy `/register` callers carry the OS hostname.
        // Match on either `id` or `name`, and prefer `display_name` (operator
        // override) over `name` for the resolved label.
        #[allow(clippy::type_complexity)]
        let rows: Vec<(String, String, Option<String>, Option<i16>, String)> = sqlx::query_as(
            "SELECT id::text, name, display_name, hue, kind FROM machines \
             WHERE id::text = ANY($1) OR name = ANY($1)",
        )
        .bind(&machine_ids)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error (machines lookup): {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
        let mut by_key: std::collections::HashMap<String, (String, Option<i16>, String)> =
            std::collections::HashMap::with_capacity(rows.len() * 2);
        for (id, name, display_name, hue, kind) in rows {
            let resolved = display_name.unwrap_or_else(|| name.clone());
            by_key.insert(id, (resolved.clone(), hue, kind.clone()));
            by_key.insert(name, (resolved, hue, kind));
        }
        for (_, s) in &mut with_ts {
            if s.machine_name.is_none()
                && let Some((name, hue, kind)) = by_key.get(&s.machine_id)
            {
                s.machine_name = Some(name.clone());
                s.machine_hue = *hue;
                s.machine_kind = Some(kind.clone());
            }
        }
    }

    // Aggregated token usage per session (CCT-94). Replaces the always-0
    // historical values; live sessions also pick up DB-aggregated counts
    // since the daemon persists every assistant turn.
    let session_ids: Vec<String> = with_ts.iter().map(|(_, s)| s.id.clone()).collect();
    if !session_ids.is_empty() {
        type TokenRow = (String, Option<i64>, Option<i64>, Option<i64>, Option<i64>);
        let rows: Vec<TokenRow> = sqlx::query_as(
            "SELECT session_id, \
                        SUM(input_tokens)::bigint, SUM(output_tokens)::bigint, \
                        SUM(cache_read_tokens)::bigint, SUM(cache_creation_tokens)::bigint \
                 FROM session_token_usage \
                 WHERE session_id = ANY($1) \
                 GROUP BY session_id",
        )
        .bind(&session_ids)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error (token usage aggregate): {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
        let mut by_session: std::collections::HashMap<String, cctui_proto::models::TokenUsage> =
            std::collections::HashMap::new();
        for (sid, ti, to, cr, cc) in rows {
            let cast = |v: Option<i64>| u64::try_from(v.unwrap_or(0)).unwrap_or(0);
            by_session.insert(
                sid,
                cctui_proto::models::TokenUsage {
                    tokens_in: cast(ti),
                    tokens_out: cast(to),
                    cost_usd: 0.0,
                    cache_read_tokens: cast(cr),
                    cache_creation_tokens: cast(cc),
                },
            );
        }
        for (_, s) in &mut with_ts {
            if let Some(usage) = by_session.remove(&s.id) {
                s.token_usage = usage;
            }
        }
    }

    // Last message text + timestamp per session, from stream_events.
    if !session_ids.is_empty() {
        let rows: Vec<(String, serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
            "SELECT DISTINCT ON (session_id) session_id, payload, created_at \
             FROM stream_events \
             WHERE session_id = ANY($1) AND event_type = 'message' \
             ORDER BY session_id, created_at DESC",
        )
        .bind(&session_ids)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error (last message lookup): {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
        let mut by_session: std::collections::HashMap<String, (Option<String>, DateTime<Utc>)> =
            std::collections::HashMap::new();
        for (sid, payload, ts) in rows {
            let text = payload
                .get("text")
                .and_then(|v| v.as_str())
                .or_else(|| payload.get("content").and_then(|v| v.as_str()))
                .map(normalize_last_message);
            by_session.insert(sid, (text, ts));
        }
        for (_, s) in &mut with_ts {
            if let Some((text, ts)) = by_session.remove(&s.id) {
                s.last_message_text = text;
                s.last_message_at = Some(ts);
            }
        }
    }

    // Status signals + display metadata from the session row, applied
    // uniformly to live + historical items: the ✋ attention glyph (from the
    // classifier) and the name/model/effort columns.
    if !session_ids.is_empty() {
        type SignalRow = (
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
            Option<String>,
        );
        let rows: Vec<SignalRow> = sqlx::query_as(
            "SELECT id, tempo, agent_state, activity, session_name, model, effort \
             FROM sessions WHERE id = ANY($1)",
        )
        .bind(&session_ids)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error (status signals lookup): {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
        let mut by_session: std::collections::HashMap<String, SignalRow> =
            std::collections::HashMap::new();
        for row in rows {
            by_session.insert(row.0.clone(), row);
        }
        for (_, s) in &mut with_ts {
            if let Some((_, tempo, agent_state, activity, name, model, effort)) =
                by_session.remove(&s.id)
            {
                let bucket = bucket_from_signals(
                    tempo.as_deref(),
                    agent_state.as_deref(),
                    activity.as_deref(),
                );
                s.bucket = bucket;
                s.attention = attention_from_bucket(bucket);
                // Worker exited but resumable on reply (CCT-228). The adapter
                // parks the marker in `tempo`; the next live snapshot after a
                // resume overwrites it.
                s.hibernated = tempo.as_deref() == Some("hibernated");
                s.name = name;
                s.model = model;
                s.effort = effort;
            }
        }
    }

    // Reflect the in-memory auto-approve flag per session (CCT-151) so clients
    // can render the toggle in its current state.
    {
        let store = state.permission_store.read().await;
        for (_, s) in &mut with_ts {
            s.auto_approve = store.is_auto_approve(&s.id);
        }
    }

    // Cold-cache surfacing (CCT-189). The per-message cache split lives in
    // `session_token_usage`; the SUM() aggregate above flattens it, so here we
    // pull only the *most recent* row per session to derive:
    //   - `cache_cold`     — that turn re-billed the full context
    //                        (cache_creation > 0 && cache_read == 0).
    //   - `last_activity_at` — its timestamp, so the client can predict cache
    //                        expiry (Anthropic's ~5-min sliding window) before
    //                        the next send.
    //   - `estimated_burst_tokens` — the cached-context size from the last
    //                        turn (≈ cache_read + cache_creation), i.e. how
    //                        many tokens get re-written on the next send.
    if !session_ids.is_empty() {
        type LastRow = (String, i64, i64, DateTime<Utc>);
        let rows: Vec<LastRow> = sqlx::query_as(
            "SELECT DISTINCT ON (session_id) session_id, \
                    cache_read_tokens, cache_creation_tokens, created_at \
             FROM session_token_usage \
             WHERE session_id = ANY($1) \
             ORDER BY session_id, created_at DESC",
        )
        .bind(&session_ids)
        .fetch_all(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error (last token usage lookup): {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
        let mut by_session: std::collections::HashMap<String, (i64, i64, DateTime<Utc>)> =
            std::collections::HashMap::new();
        for (sid, cr, cc, ts) in rows {
            by_session.insert(sid, (cr, cc, ts));
        }
        for (_, s) in &mut with_ts {
            if let Some((cr, cc, ts)) = by_session.remove(&s.id) {
                s.last_activity_at = Some(ts);
                s.cache_cold = cc > 0 && cr == 0;
                // Context size that would be re-written to cache on the next
                // send (≈ the full cached prefix from the last turn).
                let burst_tokens = u64::try_from(cr.saturating_add(cc)).unwrap_or(0);
                if burst_tokens > 0 {
                    s.estimated_burst_tokens = Some(burst_tokens);
                }
            }
        }
    }

    // Sort by most recent message so active sessions float to the top;
    // fall back to registration time when a session has no messages yet.
    with_ts.sort_by(|a, b| {
        let key = |s: &SessionListItem, reg: DateTime<Utc>| s.last_message_at.unwrap_or(reg);
        key(&b.1, b.0).cmp(&key(&a.1, a.0))
    });
    Ok(with_ts.into_iter().map(|(_, s)| s).collect())
}

/// Query params for `GET /sessions/search` (CCT-184). `q` is a substring
/// matched (case-insensitively, trgm-accelerated) against a session's full
/// transcript plus its id/name/working-dir. `include_archived` sets the scope:
/// `false` searches live (non-archived) sessions only, `true` searches all.
/// With an empty `q` and `include_archived=true` this becomes "browse the
/// archive" — archived sessions, newest first. Offset pagination throughout.
#[derive(Debug, Default, Deserialize)]
pub struct SearchParams {
    #[serde(default)]
    pub q: String,
    #[serde(default)]
    pub include_archived: bool,
    pub limit: Option<i64>,
    pub offset: Option<i64>,
}

/// Shared SELECT + machine-name join for the session-search queries.
const SEARCH_SELECT: &str = "SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.status, \
            s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, \
            COALESCE(m.display_name, m.name) AS resolved_machine_name, \
            m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind \
     FROM sessions s \
     LEFT JOIN machines m ON m.id = s.machine_uuid";

const SEARCH_DEFAULT_LIMIT: i64 = 100;
const SEARCH_MAX_LIMIT: i64 = 500;

/// Escape LIKE/ILIKE wildcards so a user's literal `%`/`_` aren't treated as
/// pattern metacharacters, then wrap in `%…%` for a substring match. `\` is
/// the default ILIKE escape char.
fn ilike_contains(q: &str) -> String {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Split a query into terms (CCT-187): whitespace-separated, but a `"…"`-quoted
/// span is one exact term (spaces preserved). Capped at 8 terms. Terms are
/// AND-matched; the webui uses the same split to highlight every term.
fn tokenize_query(q: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for c in q.chars() {
        match c {
            '"' => {
                if !cur.is_empty() {
                    terms.push(std::mem::take(&mut cur));
                }
                in_quote = !in_quote;
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    terms.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        terms.push(cur);
    }
    terms.truncate(8);
    terms
}

/// Build a ~200-char snippet of `text` centered on the earliest case-insensitive
/// occurrence of any `needle`, so the UI can show why a session matched.
fn make_snippet(text: &str, needles: &[String]) -> String {
    const WINDOW: usize = 200;
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let hay = collapsed.to_lowercase();
    let chars: Vec<char> = collapsed.chars().collect();
    // Earliest byte hit among all needles → char offset to center on.
    let match_byte = needles.iter().filter_map(|n| hay.find(&n.to_lowercase())).min().unwrap_or(0);
    let match_char = collapsed[..match_byte].chars().count();
    let start = match_char.saturating_sub(WINDOW / 2);
    let end = (start + WINDOW).min(chars.len());
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < chars.len() {
        out.push('…');
    }
    out
}

/// `GET /sessions/search?q=…&include_archived=…&limit=…&offset=…` (CCT-184).
/// Full-transcript substring search, scoped to live or all sessions; with an
/// empty `q` and `include_archived=true` it browses the archive. Returns the
/// same `SessionListItem` shape as the list (so clients reuse `SessionCard`),
/// each carrying a `match_snippet` when the hit was in the transcript.
#[allow(clippy::too_many_lines)]
pub async fn search_sessions(
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SessionListResponse>, (StatusCode, Json<ApiError>)> {
    let terms = tokenize_query(params.q.trim());
    let browse = terms.is_empty();
    // No terms + live-only scope: nothing to do — the bucketed list owns the
    // live view. No terms + archived scope means "browse the archive".
    if browse && !params.include_archived {
        return Ok(Json(SessionListResponse { sessions: vec![] }));
    }
    let patterns: Vec<String> = terms.iter().map(|t| ilike_contains(t)).collect();
    let limit = params.limit.unwrap_or(SEARCH_DEFAULT_LIMIT).clamp(1, SEARCH_MAX_LIMIT);
    let offset = params.offset.unwrap_or(0).max(0);

    let rows: Vec<DbSession> = if browse {
        // Browse the archive: archived sessions only, newest first, paginated.
        let sql = format!(
            "{SEARCH_SELECT} WHERE s.status = 'archived' ORDER BY s.registered_at DESC LIMIT $1 OFFSET $2"
        );
        sqlx::query_as(&sql).bind(limit).bind(offset).fetch_all(&state.pool).await
    } else {
        // AND across terms: a session matches only if EVERY term hits somewhere
        // — its transcript (trgm-accelerated EXISTS) or its id / name / dir.
        // Terms may hit in different events; that's the "convo contains all the
        // words" model. The scope clause keeps archived rows out unless asked.
        let scope = if params.include_archived { "TRUE" } else { "s.status <> 'archived'" };
        let clauses: Vec<String> = (1..=patterns.len())
            .map(|i| {
                format!(
                    "(s.id ILIKE ${i} OR COALESCE(s.session_name, '') ILIKE ${i} \
                      OR s.working_dir ILIKE ${i} \
                      OR EXISTS (SELECT 1 FROM stream_events e \
                                 WHERE e.session_id::text = s.id AND e.search_text ILIKE ${i}))"
                )
            })
            .collect();
        let (li, oi) = (patterns.len() + 1, patterns.len() + 2);
        let sql = format!(
            "{SEARCH_SELECT} WHERE ({scope}) AND {} ORDER BY s.registered_at DESC LIMIT ${li} OFFSET ${oi}",
            clauses.join(" AND "),
        );
        let mut query = sqlx::query_as::<_, DbSession>(&sql);
        for p in &patterns {
            query = query.bind(p);
        }
        query.bind(limit).bind(offset).fetch_all(&state.pool).await
    }
    .map_err(|e| {
        tracing::error!("db error (session search): {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    let with_ts: Vec<(DateTime<Utc>, SessionListItem)> = rows
        .into_iter()
        .map(|row| {
            let (status, liveness) =
                resolve_status_liveness(&row.status, row.registered_at, row.last_heartbeat);
            (
                row.registered_at,
                SessionListItem {
                    id: row.id,
                    parent_id: row.parent_id,
                    machine_id: row.machine_id,
                    working_dir: row.working_dir,
                    status,
                    liveness,
                    attention: None,
                    bucket: Bucket::Working,
                    uptime_secs: (Utc::now() - row.registered_at).num_seconds(),
                    token_usage: cctui_proto::models::TokenUsage::default(),
                    metadata: row.metadata,
                    adapter_id: row.adapter_id.map(cctui_proto::adapter::AdapterId::new),
                    machine_name: row.resolved_machine_name,
                    machine_hue: row.resolved_machine_hue,
                    machine_kind: row.resolved_machine_kind,
                    last_message_text: None,
                    last_message_at: None,
                    name: None,
                    model: None,
                    effort: None,
                    auto_approve: false,
                    match_snippet: None,
                    last_activity_at: None,
                    cache_cold: false,
                    estimated_burst_tokens: None,
                    hibernated: false,
                },
            )
        })
        .collect();

    let mut sessions = enrich_and_sort(&state, with_ts).await?;

    // Attach a transcript snippet per session: the most recent matching event's
    // searchable text, windowed around the keyword. Sessions matched only by
    // id/name/dir have no transcript hit and keep `match_snippet = None`.
    let ids: Vec<String> = sessions.iter().map(|s| s.id.clone()).collect();
    if !browse && !ids.is_empty() {
        // Snippet from the most recent event matching ANY term ($1 = ids,
        // $2.. = patterns); windowed around the earliest term in that text.
        let or = (2..=patterns.len() + 1)
            .map(|i| format!("search_text ILIKE ${i}"))
            .collect::<Vec<_>>()
            .join(" OR ");
        let sql = format!(
            "SELECT DISTINCT ON (session_id) session_id::text, search_text \
             FROM stream_events \
             WHERE session_id::text = ANY($1) AND ({or}) \
             ORDER BY session_id, created_at DESC"
        );
        let mut query = sqlx::query_as::<_, (String, String)>(&sql).bind(&ids);
        for p in &patterns {
            query = query.bind(p);
        }
        let snippet_rows = query.fetch_all(&state.pool).await.map_err(|e| {
            tracing::error!("db error (search snippets): {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
        let mut by_session: std::collections::HashMap<String, String> =
            snippet_rows.into_iter().map(|(id, text)| (id, make_snippet(&text, &terms))).collect();
        for s in &mut sessions {
            s.match_snippet = by_session.remove(&s.id);
        }
    }

    Ok(Json(SessionListResponse { sessions }))
}

pub async fn get_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<SessionListItem>, (StatusCode, Json<ApiError>)> {
    // Live session — serve straight from the registry.
    {
        let registry = state.registry.read().await;
        if let Some(handle) = registry.get(&session_id) {
            let item = SessionListItem {
                id: handle.session.id.clone(),
                parent_id: handle.session.parent_id.clone(),
                machine_id: handle.session.machine_id.clone(),
                working_dir: handle.session.working_dir.clone(),
                status: derive_status(handle.session.registered_at, handle.session.last_heartbeat),
                liveness: derive_liveness(handle.session.last_heartbeat),
                attention: None,
                bucket: Bucket::Working,
                uptime_secs: (Utc::now() - handle.session.registered_at).num_seconds(),
                token_usage: handle.token_usage.clone(),
                metadata: handle.session.metadata.clone(),
                adapter_id: handle.session.adapter_id.clone(),
                machine_name: None,
                machine_hue: None,
                machine_kind: None,
                last_message_text: None,
                last_message_at: None,
                name: None,
                model: None,
                effort: None,
                auto_approve: state
                    .permission_store
                    .read()
                    .await
                    .is_auto_approve(&handle.session.id),
                match_snippet: None,
                last_activity_at: None,
                cache_cold: false,
                estimated_burst_tokens: None,
                hibernated: false,
            };
            return Ok(Json(item));
        }
    }

    // Not live — fall back to the DB so archived/ended sessions still open
    // (read-only). A true 404 now means the session was actually deleted, not
    // just archived (CCT-250 item 6 — kills the spurious "not found or
    // archived" toast on refresh).
    let row: Option<DbSession> = sqlx::query_as(
        "SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.status, \
                s.registered_at, s.last_heartbeat, s.metadata, s.adapter_id, \
                COALESCE(m.display_name, m.name) AS resolved_machine_name, \
                m.hue AS resolved_machine_hue, m.kind AS resolved_machine_kind \
         FROM sessions s \
         LEFT JOIN machines m ON m.id = s.machine_uuid \
         WHERE s.id = $1",
    )
    .bind(&session_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    let row = row.ok_or_else(|| {
        (StatusCode::NOT_FOUND, Json(ApiError { error: "session not found".into() }))
    })?;

    let (status, liveness) =
        resolve_status_liveness(&row.status, row.registered_at, row.last_heartbeat);
    let item = SessionListItem {
        id: row.id.clone(),
        parent_id: row.parent_id,
        machine_id: row.machine_id,
        working_dir: row.working_dir,
        status,
        liveness,
        attention: None,
        bucket: Bucket::Working,
        uptime_secs: (Utc::now() - row.registered_at).num_seconds(),
        token_usage: cctui_proto::models::TokenUsage::default(),
        metadata: row.metadata,
        adapter_id: row.adapter_id.map(cctui_proto::adapter::AdapterId::new),
        machine_name: row.resolved_machine_name,
        machine_hue: row.resolved_machine_hue,
        machine_kind: row.resolved_machine_kind,
        last_message_text: None,
        last_message_at: None,
        name: None,
        model: None,
        effort: None,
        auto_approve: state.permission_store.read().await.is_auto_approve(&row.id),
        match_snippet: None,
        last_activity_at: None,
        cache_cold: false,
        estimated_burst_tokens: None,
        hibernated: false,
    };
    Ok(Json(item))
}

pub async fn get_conversation(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<serde_json::Value>>, (StatusCode, Json<ApiError>)> {
    let adapter: Option<String> =
        sqlx::query_scalar("SELECT adapter_id FROM sessions WHERE id = $1")
            .bind(&session_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|e| {
                tracing::error!("db error: {e}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiError { error: "database error".into() }),
                )
            })?;

    let rows: Vec<(String, serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
        "SELECT event_type, payload, created_at FROM stream_events \
         WHERE session_id = $1 ORDER BY created_at ASC",
    )
    .bind(&session_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;

    let adapter_id = adapter.as_deref().unwrap_or("claude-code");
    // Stamp each event with `ts` (unix millis, matching the live `AgentEvent`
    // shape) derived from `created_at`, so the client renders real timestamps
    // instead of "Invalid Date". Legacy payloads that already carry `ts` keep
    // their own value.
    let normalized: Vec<serde_json::Value> = rows
        .into_iter()
        .filter_map(|(event_type, payload, created_at)| {
            crate::normalize::for_client(adapter_id, &event_type, payload).map(|mut v| {
                if let Some(obj) = v.as_object_mut() {
                    obj.entry("ts")
                        .or_insert_with(|| serde_json::json!(created_at.timestamp_millis()));
                }
                v
            })
        })
        .collect();
    Ok(Json(normalized))
}

pub async fn send_message(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<MessageRequest>,
) -> StatusCode {
    let dispatch = crate::daemon_dispatch::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Reply {
            local_id: session_id.clone(),
            text: req.content,
            ask_picks: None,
        },
    )
    .await;
    if let Err(err) = dispatch {
        use crate::daemon_dispatch::Error;
        match err {
            Error::NoDaemon(_) | Error::NoAdapter | Error::NotFound => {
                tracing::debug!(%session_id, ?err, "daemon dispatch skipped");
            }
            _ => tracing::warn!(%session_id, %err, "daemon dispatch failed"),
        }
    }
    StatusCode::ACCEPTED
}

pub async fn rename_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<RenameRequest>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let name = req.name.trim();
    if name.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ApiError { error: "name must not be empty".into() }),
        ));
    }
    // Persist immediately so the UI reflects the rename without waiting for
    // the daemon round-trip. The daemon write-through (below) keeps the
    // on-disk state.json in sync so the next status poll doesn't revert it.
    sqlx::query("UPDATE sessions SET session_name = $2 WHERE id = $1")
        .bind(&session_id)
        .bind(name)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    // Best-effort propagation to the owning daemon's adapter.
    let _ = crate::daemon_dispatch::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Rename {
            local_id: session_id.clone(),
            name: name.to_owned(),
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn kill_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    // Best-effort: also dispatch to the daemon so the running worker is
    // actually killed via the `claude daemon` socket. The DB update
    // below remains source-of-truth.
    let _ = crate::daemon_dispatch::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Kill { local_id: session_id.clone(), signal: None },
    )
    .await;
    // Kill drops the in-memory handle and marks the DB row inactive. The
    // session isn't archived — later activity can revive it.
    sqlx::query("UPDATE sessions SET status = 'inactive' WHERE id = $1")
        .bind(&session_id)
        .execute(&state.pool)
        .await
        .map_err(|e| {
            tracing::error!("db error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
        })?;
    {
        let mut registry = state.registry.write().await;
        registry.deregister(&session_id);
    }
    tracing::info!(session_id = %session_id, "session killed");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/interrupt` — stop the in-flight turn without
/// tearing the session down (CCT-151, CCT-210). Dispatches `Interrupt`, NOT a
/// kill: the earlier `Kill { signal: 15 }` terminated the worker on both
/// adapters (claude `kill`-op'd the PTY worker; codex sent `turn/interrupt`
/// *and then* `terminate_child`). `Interrupt` keeps the session alive — the
/// claude adapter injects an ESC keystroke into the worker PTY via `attach`,
/// the codex adapter sends `turn/interrupt` without terminating. The DB row
/// stays active and in the registry so the session keeps going.
pub async fn interrupt_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    let _ = crate::daemon_dispatch::dispatch(
        &state,
        &session_id,
        cctui_proto::adapter::AdapterCommand::Interrupt { local_id: session_id.clone() },
    )
    .await;
    tracing::info!(session_id = %session_id, "session interrupted");
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /api/v1/sessions/{id}/auto-approve` — toggle cctui-side auto-approve
/// (CCT-151). When on, incoming permission requests for this session are
/// answered `allow` immediately. In-memory; reset on server restart.
pub async fn set_auto_approve(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(req): Json<cctui_proto::api::AutoApproveRequest>,
) -> StatusCode {
    state.permission_store.write().await.set_auto_approve(&session_id, req.enabled);
    tracing::info!(session_id = %session_id, enabled = req.enabled, "auto-approve toggled");
    StatusCode::NO_CONTENT
}

/// Archive a session: dismiss it from the default list. Best-effort dispatches
/// `Remove` to the owning daemon — for claude-code that stops the worker and
/// then runs `claude rm` so the session also disappears from Claude Code's
/// native `claude agents` view (CCT-132); for codex it is a plain kill. Then
/// marks the row `archived` and drops it from the live registry. The
/// conversation transcript is preserved. Reversible (cctui-side) via
/// `unarchive_session`, though the underlying claude job is gone by then.
pub async fn archive_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    archive_one(&state, &session_id).await.map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

/// Archive a single session (+ its subagents) — the reusable core shared by the
/// single-session route and the batch route. Dispatches `Remove`, marks the row
/// `archived`, clears classifier signals, and drops it from the live registry.
async fn archive_one(state: &AppState, session_id: &str) -> Result<(), sqlx::Error> {
    let _ = crate::daemon_dispatch::dispatch(
        state,
        session_id,
        cctui_proto::adapter::AdapterCommand::Remove { local_id: session_id.to_string() },
    )
    .await;
    // Archive the session AND any Task-tool subagents nested under it
    // (CCT-141): a parent's children should never outlive it in the list.
    // Subagents are observe-only (no worker), so they need no `claude rm` —
    // only the parent does, handled by the dispatch above. Archiving a
    // *child* does not touch the parent (no `parent_id` cascade upward).
    let children: Vec<String> = sqlx::query_scalar("SELECT id FROM sessions WHERE parent_id = $1")
        .bind(session_id)
        .fetch_all(&state.pool)
        .await
        .unwrap_or_default();
    // Clear the classifier signals on archive so a session that was waiting on
    // input doesn't keep its ✋ "needs input" glyph in the archived view — an
    // archived session is, by definition, no longer waiting on anyone.
    sqlx::query(
        "UPDATE sessions SET status = 'archived', tempo = NULL, agent_state = NULL, activity = NULL \
         WHERE id = $1 OR parent_id = $1",
    )
    .bind(session_id)
    .execute(&state.pool)
    .await?;
    {
        let mut registry = state.registry.write().await;
        registry.deregister(session_id);
        for child in &children {
            registry.deregister(child);
        }
    }
    tracing::info!(session_id = %session_id, children = children.len(), "session archived");
    Ok(())
}

/// Un-archive a session: clear the sticky `archived` state back to
/// `inactive` so it reappears in the default list and can re-derive its
/// status from activity.
pub async fn unarchive_session(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ApiError>)> {
    unarchive_one(&state, &session_id).await.map_err(|e| {
        tracing::error!("db error: {e}");
        (StatusCode::INTERNAL_SERVER_ERROR, Json(ApiError { error: "database error".into() }))
    })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn unarchive_one(state: &AppState, session_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE sessions SET status = 'inactive' WHERE id = $1 AND status = 'archived'")
        .bind(session_id)
        .execute(&state.pool)
        .await?;
    tracing::info!(session_id = %session_id, "session unarchived");
    Ok(())
}

/// Body for the batch archive/unarchive routes.
#[derive(Deserialize)]
pub struct BatchIds {
    pub ids: Vec<String>,
}

/// `POST /api/v1/sessions/archive` — archive many sessions in one request
/// (CCT-172, multi-select). Each id is processed via [`archive_one`]; a per-id
/// failure is logged but does not abort the rest of the batch. Idempotent:
/// re-archiving an already-archived id is a no-op.
pub async fn archive_sessions(
    State(state): State<AppState>,
    Json(req): Json<BatchIds>,
) -> StatusCode {
    let mut ok = 0usize;
    for id in &req.ids {
        match archive_one(&state, id).await {
            Ok(()) => ok += 1,
            Err(e) => tracing::error!(session_id = %id, "batch archive db error: {e}"),
        }
    }
    tracing::info!(archived = ok, requested = req.ids.len(), "batch archive");
    StatusCode::NO_CONTENT
}

/// `POST /api/v1/sessions/unarchive` — the batch mirror of `unarchive_session`.
pub async fn unarchive_sessions(
    State(state): State<AppState>,
    Json(req): Json<BatchIds>,
) -> StatusCode {
    let mut ok = 0usize;
    for id in &req.ids {
        match unarchive_one(&state, id).await {
            Ok(()) => ok += 1,
            Err(e) => tracing::error!(session_id = %id, "batch unarchive db error: {e}"),
        }
    }
    tracing::info!(unarchived = ok, requested = req.ids.len(), "batch unarchive");
    StatusCode::NO_CONTENT
}

pub async fn set_session_policy(
    State(state): State<AppState>,
    Path(session_id): Path<String>,
    Json(rules): Json<Vec<crate::policy::PolicyRule>>,
) -> StatusCode {
    let mut registry = state.registry.write().await;
    registry.set_policy(&session_id, rules);
    StatusCode::OK
}

/// CCT-110: normalize a session's last-message text for the sessions
/// table. Collapses every run of ASCII/Unicode whitespace (newlines,
/// tabs, NBSP) into a single space so multi-line snippets don't blow
/// up row height when CSS doesn't fully suppress wrapping, and caps at
/// 200 chars + ellipsis as a backstop. CSS handles the *visual* truncation
/// to the column's actual rendered width (`text-overflow: ellipsis`).
fn normalize_last_message(s: &str) -> String {
    const MAX_CHARS: usize = 200;
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut iter = collapsed.chars();
    let head: String = iter.by_ref().take(MAX_CHARS).collect();
    if iter.next().is_some() { format!("{head}…") } else { head }
}

#[cfg(test)]
mod tests {
    use super::{
        attention_from_bucket, bucket_from_signals, derive_liveness, normalize_last_message,
    };

    fn attention_from_signals(
        tempo: Option<&str>,
        agent_state: Option<&str>,
        activity: Option<&str>,
    ) -> Option<Attention> {
        attention_from_bucket(bucket_from_signals(tempo, agent_state, activity))
    }
    use cctui_proto::models::{Attention, Liveness};
    use chrono::{Duration, Utc};

    #[test]
    fn liveness_active_within_active_window() {
        let hb = Utc::now() - Duration::seconds(60);
        assert_eq!(derive_liveness(hb), Liveness::Active);
    }

    #[test]
    fn liveness_stale_between_windows() {
        let hb = Utc::now() - Duration::minutes(20);
        assert_eq!(derive_liveness(hb), Liveness::Stale);
    }

    #[test]
    fn liveness_dead_past_dead_window() {
        let hb = Utc::now() - Duration::hours(3);
        assert_eq!(derive_liveness(hb), Liveness::Dead);
    }

    #[test]
    fn attention_needs_input_when_tempo_blocked() {
        assert_eq!(
            attention_from_signals(Some("blocked"), Some("working"), None),
            Some(Attention::NeedsInput),
        );
    }

    #[test]
    fn no_attention_when_tempo_active() {
        assert_eq!(attention_from_signals(Some("active"), Some("working"), None), None);
    }

    #[test]
    fn no_attention_when_blocked_but_already_failed() {
        // activity=failure short-circuits to Done before the blocked check.
        assert_eq!(attention_from_signals(Some("blocked"), None, Some("failure")), None);
    }

    #[test]
    fn no_attention_when_no_signals() {
        assert_eq!(attention_from_signals(None, None, None), None);
    }

    #[test]
    fn bucket_reflects_signals() {
        use cctui_proto::classifier::Bucket;
        assert_eq!(bucket_from_signals(Some("blocked"), Some("working"), None), Bucket::Blocked);
        assert_eq!(bucket_from_signals(Some("active"), None, None), Bucket::Working);
        assert_eq!(bucket_from_signals(None, Some("stopped"), None), Bucket::Done);
        assert_eq!(bucket_from_signals(None, None, Some("success")), Bucket::Done);
    }

    #[test]
    fn collapses_newlines_and_runs_of_whitespace() {
        let raw = "line one\n\nline two\t\t  line three";
        assert_eq!(normalize_last_message(raw), "line one line two line three");
    }

    #[test]
    fn truncates_with_ellipsis_past_max() {
        let raw = "a".repeat(250);
        let out = normalize_last_message(&raw);
        // 200 chars + the ellipsis.
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }

    #[test]
    fn does_not_ellipsis_short_text() {
        let raw = "short message";
        assert_eq!(normalize_last_message(raw), "short message");
    }

    #[test]
    fn handles_cjk_and_emoji_by_char_count_not_bytes() {
        // 200 of these = 200 chars, well under the cap.
        let raw = "漢".repeat(200);
        let out = normalize_last_message(&raw);
        assert_eq!(out.chars().count(), 200);
        assert!(!out.ends_with('…'));
    }

    #[test]
    fn truncates_cjk_at_char_boundary_not_byte() {
        // 250 CJK chars (3 bytes each in UTF-8 = 750 bytes).
        let raw = "漢".repeat(250);
        let out = normalize_last_message(&raw);
        assert_eq!(out.chars().count(), 201);
        assert!(out.ends_with('…'));
    }
}
