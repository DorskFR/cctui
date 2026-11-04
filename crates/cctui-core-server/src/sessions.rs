//! The caller's own session list, read by the worker liveness probe.
use crate::host::Host;
use axum::extract::State;
use axum::{Extension, Json};
use cctui_proto::models::SessionStatus;
use cctui_server_core as core;
use chrono::{DateTime, Utc};
use core::auth::{AuthContext, BoundSession};
use core::error::AppError;
use core::store::sessions::SessionRowStatus;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

const STATUS_WINDOW_SECS: i64 = 5 * 60;

#[derive(Debug, Serialize)]
pub struct SessionList {
    pub sessions: Vec<SessionEntry>,
}

#[derive(Debug, Serialize)]
pub struct SessionEntry {
    pub id: String,
    pub parent_id: Option<String>,
    pub machine_id: String,
    pub working_dir: String,
    pub adapter_id: String,
    pub status: SessionStatus,
    pub registered_at: DateTime<Utc>,
    pub last_heartbeat: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct Row {
    id: String,
    parent_id: Option<String>,
    machine_id: String,
    working_dir: String,
    adapter_id: String,
    status: String,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
}

pub async fn list(
    State(host): State<Host>,
    Extension(ctx): Extension<AuthContext>,
    worker: Option<Extension<BoundSession>>,
) -> Result<Json<SessionList>, AppError> {
    let bound = worker.map(|Extension(BoundSession(id))| id);
    let sessions = visible_sessions(&host.pool, ctx.owner_filter(), bound.as_deref()).await?;
    Ok(Json(SessionList { sessions }))
}

async fn visible_sessions(
    pool: &PgPool,
    owner: Option<Uuid>,
    bound: Option<&str>,
) -> Result<Vec<SessionEntry>, sqlx::Error> {
    let sql = format!(
        "SELECT s.id, s.parent_id, s.machine_id, s.working_dir, s.adapter_id, s.status, \
                s.registered_at, s.last_heartbeat \
         FROM sessions s \
         WHERE {} \
         AND ($1::uuid IS NULL OR s.user_id = $1) \
         AND ($2::text IS NULL OR s.id = $2) \
         ORDER BY s.registered_at DESC",
        cctui_server_core::live_sessions_predicate!("s"),
    );
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(owner)
        .bind(bound)
        .fetch_all(pool)
        .await?;
    let now = Utc::now();
    Ok(rows
        .into_iter()
        .map(|row| SessionEntry {
            status: wire_status(&row.status, row.registered_at, row.last_heartbeat, now),
            id: row.id,
            parent_id: row.parent_id,
            machine_id: row.machine_id,
            working_dir: row.working_dir,
            adapter_id: row.adapter_id,
            registered_at: row.registered_at,
            last_heartbeat: row.last_heartbeat,
        })
        .collect())
}

fn wire_status(
    row_status: &str,
    registered_at: DateTime<Utc>,
    last_heartbeat: DateTime<Utc>,
    now: DateTime<Utc>,
) -> SessionStatus {
    SessionRowStatus::parse(row_status).filter(|s| s.is_sticky()).map_or_else(
        || {
            if (now - last_heartbeat).num_seconds() < STATUS_WINDOW_SECS {
                SessionStatus::Active
            } else if (now - registered_at).num_seconds() < STATUS_WINDOW_SECS {
                SessionStatus::New
            } else {
                SessionStatus::Inactive
            }
        },
        SessionRowStatus::to_wire,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_server_core::test_support::with_isolated_database;
    use chrono::Duration;

    fn ago(now: DateTime<Utc>, secs: i64) -> DateTime<Utc> {
        now - Duration::seconds(secs)
    }

    /// The worker entrypoint's `probe_session` reading of one listed row.
    fn probe_reading(status: SessionStatus) -> &'static str {
        match serde_json::to_value(status).unwrap().as_str().unwrap() {
            "active" | "new" => "registered",
            "inactive" => "inactive",
            _ => "unknown",
        }
    }

    #[test]
    fn status_follows_heartbeat_and_registration_age() {
        let now = Utc::now();
        let status = |row, reg, hb| wire_status(row, ago(now, reg), ago(now, hb), now);
        assert_eq!(status("active", 3600, 10), SessionStatus::Active);
        assert_eq!(status("new", 30, 30), SessionStatus::Active);
        assert_eq!(status("active", 60, 600), SessionStatus::New);
        assert_eq!(status("active", 3600, 600), SessionStatus::Inactive);
        assert_eq!(status("inactive", 3600, 10), SessionStatus::Active);
        assert_eq!(status("ended", 60, 10), SessionStatus::Inactive);
        assert_eq!(status("failed", 60, 10), SessionStatus::Inactive);
        assert_eq!(status("archived", 60, 10), SessionStatus::Archived);
    }

    #[test]
    fn the_probe_reads_live_rows_as_registered_and_ended_rows_as_inactive() {
        let now = Utc::now();
        let live = wire_status("active", ago(now, 30), ago(now, 5), now);
        let booting = wire_status("active", ago(now, 30), ago(now, 400), now);
        let ended = wire_status("ended", ago(now, 30), ago(now, 5), now);
        assert_eq!(probe_reading(live), "registered");
        assert_eq!(probe_reading(booting), "registered");
        assert_eq!(probe_reading(ended), "inactive");
    }

    #[test]
    fn last_heartbeat_serializes_in_the_form_the_probe_parses() {
        for micros in [0, 120_000, 123_456] {
            let hb = DateTime::from_timestamp(1_790_000_000, micros * 1000).unwrap();
            let entry = SessionEntry {
                id: "s".into(),
                parent_id: None,
                machine_id: "m".into(),
                working_dir: "/w".into(),
                adapter_id: "claude-code".into(),
                status: SessionStatus::Active,
                registered_at: hb,
                last_heartbeat: hb,
            };
            let json = serde_json::to_value(SessionList { sessions: vec![entry] }).unwrap();
            let raw = json["sessions"][0]["last_heartbeat"].as_str().unwrap();
            assert!(raw.ends_with('Z'), "{raw}");
            let whole = raw.split('.').next().unwrap().trim_end_matches('Z');
            let parsed = chrono::NaiveDateTime::parse_from_str(whole, "%Y-%m-%dT%H:%M:%S")
                .unwrap_or_else(|e| panic!("{raw}: {e}"));
            assert_eq!(parsed.and_utc().timestamp(), 1_790_000_000);
            assert_eq!(json["sessions"][0]["status"], "active");
            assert_eq!(json["sessions"][0]["id"], "s");
        }
    }

    async fn seed(pool: &PgPool) -> (Uuid, Uuid) {
        let mut conn = core::schema::migration_connection(pool).await.unwrap();
        core::schema::provision(&mut conn).await.unwrap();
        drop(conn);
        let (owner, other) = (Uuid::new_v4(), Uuid::new_v4());
        let (machine, foreign) = (Uuid::new_v4(), Uuid::new_v4());
        for (user, m) in [(owner, machine), (other, foreign)] {
            sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $2)")
                .bind(user)
                .bind(format!("u-{user}"))
                .execute(pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $3)",
            )
            .bind(m)
            .bind(user)
            .bind(format!("m-{m}"))
            .execute(pool)
            .await
            .unwrap();
        }
        let rows = [
            ("worker", owner, machine, "active"),
            ("sibling", owner, machine, "active"),
            ("done", owner, machine, "ended"),
            ("shelved", owner, machine, "archived"),
            ("foreign", other, foreign, "active"),
        ];
        for (id, user, m, status) in rows {
            sqlx::query(
                "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid, status) \
                 VALUES ($1, $2, '/w', $3, $4, $5)",
            )
            .bind(id)
            .bind(m.to_string())
            .bind(user)
            .bind(m)
            .bind(status)
            .execute(pool)
            .await
            .unwrap();
        }
        (owner, other)
    }

    fn ids(sessions: &[SessionEntry]) -> Vec<&str> {
        let mut ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        ids.sort_unstable();
        ids
    }

    #[tokio::test]
    async fn a_machine_key_lists_its_owners_live_sessions_with_probe_fields() {
        with_isolated_database("core-sessions-machine", |pool| async move {
            let (owner, _) = seed(&pool).await;
            let sessions = visible_sessions(&pool, Some(owner), None).await.unwrap();
            assert_eq!(ids(&sessions), ["done", "sibling", "worker"]);
            let worker = sessions.iter().find(|s| s.id == "worker").unwrap();
            assert_eq!(probe_reading(worker.status), "registered");
            assert!((Utc::now() - worker.last_heartbeat).num_seconds() < 60);
            let done = sessions.iter().find(|s| s.id == "done").unwrap();
            assert_eq!(probe_reading(done.status), "inactive");
        })
        .await;
    }

    #[tokio::test]
    async fn a_worker_key_lists_only_its_own_session() {
        with_isolated_database("core-sessions-worker", |pool| async move {
            let (owner, other) = seed(&pool).await;
            let own = visible_sessions(&pool, Some(owner), Some("worker")).await.unwrap();
            assert_eq!(ids(&own), ["worker"]);
            let foreign = visible_sessions(&pool, Some(other), Some("worker")).await.unwrap();
            assert!(foreign.is_empty(), "a bound key never sees another owner's session");
            let admin = visible_sessions(&pool, None, None).await.unwrap();
            assert_eq!(ids(&admin), ["done", "foreign", "sibling", "worker"]);
        })
        .await;
    }
}
