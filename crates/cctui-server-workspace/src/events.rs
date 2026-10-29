//! Lifecycle audit services shared with the automation host.
use crate::state::AppState;
pub use cctui_server_core::events::{
    Actor, Event, EventRecord, Severity, is_system_kind, kind, prune,
};
#[cfg(test)]
use cctui_server_core::events::{
    DEFAULT_RETENTION_DAYS, RETENTION_KEY, render_summary, retention_days,
};

pub fn record(state: &AppState, event: Event) {
    cctui_server_core::events::record(&state.pool, &state.bus, event);
}

pub async fn record_now(state: &AppState, event: Event) -> Result<EventRecord, sqlx::Error> {
    cctui_server_core::events::record_now(&state.pool, &state.bus, event).await
}

#[cfg(test)]
use serde_json::{Value, json};
#[cfg(test)]
use uuid::Uuid;

#[cfg(test)]
pub mod tests {
    use super::*;

    fn ev(kind: &str) -> Event {
        Event::new(kind, Actor::System)
    }

    #[test]
    fn session_summaries_prefer_the_name_and_fall_back_to_the_short_id() {
        let named = ev(kind::SESSION_KILLED)
            .session("0123456789abcdef")
            .detail(json!({ "session_name": "deploy prod" }));
        assert_eq!(render_summary(kind::SESSION_KILLED, &named), "deploy prod killed");
        let anon = ev(kind::SESSION_KILLED).session("0123456789abcdef");
        assert_eq!(render_summary(kind::SESSION_KILLED, &anon), "01234567 killed");
    }

    #[test]
    fn ended_carries_the_reason_and_the_first_detail_line() {
        let e = ev(kind::SESSION_ENDED).session("s").detail(json!({
            "session_name": "x",
            "end_reason": "crashed",
            "end_detail": "exit 1\nstderr tail",
        }));
        assert_eq!(render_summary(kind::SESSION_ENDED, &e), "x ended (crashed): exit 1");
    }

    #[test]
    fn machine_offline_counts_the_sessions_it_took_down() {
        let id = Uuid::from_u128(1);
        let e = ev(kind::MACHINE_OFFLINE)
            .machine(id)
            .detail(json!({ "machine_label": "nas", "ended_sessions": 4 }));
        assert_eq!(render_summary(kind::MACHINE_OFFLINE, &e), "nas offline, 4 sessions ended");
        let quiet = ev(kind::MACHINE_OFFLINE).machine(id).detail(json!({ "machine_label": "nas" }));
        assert_eq!(render_summary(kind::MACHINE_OFFLINE, &quiet), "nas offline");
    }

    #[test]
    fn auto_resume_distinguishes_a_nudge_from_exhaustion() {
        let nudge = ev(kind::SESSION_AUTO_RESUMED)
            .session("abcdefgh1")
            .detail(json!({ "attempt": 2, "max_attempts": 3 }));
        assert_eq!(
            render_summary(kind::SESSION_AUTO_RESUMED, &nudge),
            "abcdefgh auto-resume nudge 2/3"
        );
        let done = ev(kind::SESSION_AUTO_RESUMED)
            .session("abcdefgh1")
            .detail(json!({ "exhausted": true, "attempts": 3 }));
        assert_eq!(
            render_summary(kind::SESSION_AUTO_RESUMED, &done),
            "abcdefgh auto-resume gave up after 3 attempts"
        );
    }

    #[test]
    fn unknown_kinds_render_generically_with_their_subject() {
        let e = ev("session.something_new").session("abcdefgh1");
        assert_eq!(render_summary("session.something_new", &e), "abcdefgh session.something_new");
        let m = ev("machine.rebooted").machine(Uuid::from_u128(0xabcd_ef01_2345));
        assert!(render_summary("machine.rebooted", &m).ends_with(" machine.rebooted"));
        let s = ev("system.whatever");
        assert_eq!(render_summary("system.whatever", &s), "system.whatever");
    }

    #[test]
    fn reaper_and_migrations_summaries_read_their_counts() {
        let r = ev(kind::SYSTEM_REAPER_RAN).detail(json!({ "archived": 3, "events_pruned": 10 }));
        assert_eq!(
            render_summary(kind::SYSTEM_REAPER_RAN, &r),
            "reaper archived 3 sessions, pruned 10 events"
        );
        let m = ev(kind::SYSTEM_MIGRATIONS_APPLIED).detail(json!({ "versions": [167, 168] }));
        assert_eq!(
            render_summary(kind::SYSTEM_MIGRATIONS_APPLIED, &m),
            "migrations applied: 167, 168"
        );
    }

    #[test]
    fn detail_merges_objects_and_later_keys_win() {
        let e = ev("x").detail(json!({ "a": 1, "b": 1 })).detail(json!({ "b": 2 }));
        assert_eq!(e.detail, json!({ "a": 1, "b": 2 }));
    }

    #[test]
    fn actors_serialise_to_the_documented_strings() {
        let id = Uuid::from_u128(7);
        assert_eq!(Actor::User(id).to_string(), format!("user:{id}"));
        assert_eq!(Actor::Daemon.to_string(), "daemon");
        assert_eq!(Actor::Reaper.to_string(), "reaper");
        assert_eq!(Actor::System.to_string(), "system");
        assert_eq!(Actor::Agent("s1".into()).to_string(), "agent:s1");
    }

    #[test]
    fn system_kinds_are_the_admin_only_family() {
        assert!(is_system_kind(kind::SYSTEM_SERVER_STARTED));
        assert!(!is_system_kind(kind::SESSION_ENDED));
        assert!(!is_system_kind(kind::MACHINE_OFFLINE));
    }

    #[test]
    fn severity_round_trips() {
        for s in [Severity::Info, Severity::Warn, Severity::Error] {
            assert_eq!(Severity::parse(s.as_str()), Some(s));
        }
        assert_eq!(Severity::parse("loud"), None);
    }

    async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
        let url = crate::routes::gateway::test_db_url(test_name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(2)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    pub async fn seed_user_machine(pool: &sqlx::PgPool, tag: &str) -> (Uuid, Uuid) {
        let uid = Uuid::new_v4();
        let machine = Uuid::new_v4();
        sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
            .bind(uid)
            .bind(format!("{tag}-{uid}"))
            .bind(format!("kh-{tag}-{uid}"))
            .execute(pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO machines (id, user_id, name, key_hash, display_name) \
             VALUES ($1, $2, 'host', $3, 'Lab box')",
        )
        .bind(machine)
        .bind(uid)
        .bind(format!("mk-{tag}-{machine}"))
        .execute(pool)
        .await
        .unwrap();
        (uid, machine)
    }

    pub async fn seed_session(
        pool: &sqlx::PgPool,
        uid: Uuid,
        machine: Uuid,
        name: Option<&str>,
    ) -> String {
        let sid = Uuid::new_v4().to_string();
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
                                   session_name, adapter_id) \
             VALUES ($1, $2, $2, $3, '/w', 'active', $4, 'claude-code')",
        )
        .bind(&sid)
        .bind(machine)
        .bind(uid)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
        sid
    }

    #[derive(sqlx::FromRow)]
    pub struct Row {
        pub kind: String,
        pub session_id: Option<String>,
        pub machine_id: Option<Uuid>,
        pub user_id: Option<Uuid>,
        pub actor: String,
        pub summary: String,
        pub detail: Value,
    }

    pub async fn rows_for_session(pool: &sqlx::PgPool, session_id: &str) -> Vec<Row> {
        sqlx::query_as(
            "SELECT kind, session_id, machine_id, user_id, actor, summary, detail FROM events \
             WHERE session_id = $1 OR detail->>'session_id' = $1 ORDER BY id",
        )
        .bind(session_id)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_session_event_is_enriched_with_its_owner_machine_and_name() {
        let Some(pool) = test_pool("events_enrich_session").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-enrich").await;
        let sid = seed_session(&pool, uid, machine, Some("deploy prod")).await;
        let state = AppState::for_test(pool.clone());
        record_now(&state, Event::new(kind::SESSION_KILLED, Actor::User(uid)).session(&sid))
            .await
            .expect("insert");
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.kind, kind::SESSION_KILLED);
        assert_eq!(row.machine_id, Some(machine));
        assert_eq!(row.user_id, Some(uid));
        assert_eq!(row.actor, format!("user:{uid}"));
        assert_eq!(row.summary, "deploy prod killed");
        assert_eq!(row.detail["machine_label"], "Lab box");
    }

    #[tokio::test]
    async fn deleting_the_session_keeps_the_row_with_a_null_subject() {
        let Some(pool) = test_pool("events_survive_session_delete").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-del").await;
        let sid = seed_session(&pool, uid, machine, Some("gone soon")).await;
        let state = AppState::for_test(pool.clone());
        record_now(&state, Event::new(kind::SESSION_ENDED, Actor::Daemon).session(&sid))
            .await
            .expect("insert");
        sqlx::query("DELETE FROM sessions WHERE id = $1").bind(&sid).execute(&pool).await.unwrap();
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, None);
        assert!(rows[0].summary.starts_with("gone soon ended"));
        assert_eq!(rows[0].detail["session_id"], sid);
    }

    #[tokio::test]
    async fn a_failed_insert_is_reported_by_record_now_and_swallowed_by_record() {
        let Some(pool) = test_pool("events_insert_failure").await else { return };
        let state = AppState::for_test(pool.clone());
        let orphan = Event::new(kind::MACHINE_OFFLINE, Actor::System).machine(Uuid::new_v4());
        assert!(record_now(&state, orphan.clone()).await.is_err());
        record(&state, orphan);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM events WHERE machine_id IS NULL AND kind = $1 AND actor = 'system' AND occurred_at > now() - interval '1 second'")
                .bind(kind::MACHINE_OFFLINE)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0);
    }

    fn user_ctx(uid: Uuid) -> axum::Extension<crate::auth::AuthContext> {
        axum::Extension(crate::auth::AuthContext {
            user_id: uid,
            key_id: uid,
            machine_id: None,
            scopes: [crate::auth::Scope::Read, crate::auth::Scope::Dispatch].into_iter().collect(),
        })
    }

    async fn settle() {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }

    fn kinds(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(|r| r.kind.as_str()).collect()
    }

    /// A daemon channel on the bus so dispatch-gated routes go through.
    ///
    /// The receiver is kept so the channel stays open.
    fn fake_daemon(
        state: &AppState,
        machine: Uuid,
    ) -> tokio::sync::mpsc::Receiver<cctui_proto::ws::DaemonFrameDown> {
        let (tx, rx) = tokio::sync::mpsc::channel(8);
        state.bus.register_daemon(machine, Uuid::new_v4(), tx);
        rx
    }

    #[tokio::test]
    async fn kill_records_exactly_one_row_with_the_user_as_actor() {
        use axum::extract::{Path, State};
        let Some(pool) = test_pool("events_kill_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-kill").await;
        let sid = seed_session(&pool, uid, machine, Some("ship it")).await;
        let state = AppState::for_test(pool.clone());
        crate::routes::sessions::kill_session(State(state), user_ctx(uid), Path(sid.clone()))
            .await
            .expect("kill");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_KILLED]);
        assert_eq!(rows[0].actor, format!("user:{uid}"));
        assert_eq!(rows[0].summary, "ship it killed");
        assert_eq!(rows[0].user_id, Some(uid));
        assert_eq!(rows[0].machine_id, Some(machine));
    }

    #[tokio::test]
    async fn archive_then_unarchive_record_one_row_each() {
        use axum::extract::{Path, Query, State};
        let Some(pool) = test_pool("events_archive_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-arch").await;
        let sid = seed_session(&pool, uid, machine, Some("old work")).await;
        let state = AppState::for_test(pool.clone());
        crate::routes::sessions::archive_session(
            State(state.clone()),
            user_ctx(uid),
            Path(sid.clone()),
            Query(crate::routes::sessions::ArchiveQuery::default()),
        )
        .await
        .expect("archive");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_ARCHIVED]);
        assert_eq!(rows[0].summary, "old work archived");
        assert_eq!(rows[0].detail["initiator"], "user");

        crate::routes::sessions::unarchive_session(
            State(state.clone()),
            user_ctx(uid),
            Path(sid.clone()),
        )
        .await
        .expect("unarchive");
        crate::routes::sessions::unarchive_session(State(state), user_ctx(uid), Path(sid.clone()))
            .await
            .expect("unarchive again");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(
            kinds(&rows),
            vec![kind::SESSION_ARCHIVED, kind::SESSION_UNARCHIVED],
            "a second unarchive of an already-live session changes nothing"
        );
    }

    #[tokio::test]
    async fn fork_records_one_row_on_the_parent_naming_the_child() {
        use axum::extract::{Path, State};
        let Some(pool) = test_pool("events_fork_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-fork").await;
        let sid = seed_session(&pool, uid, machine, Some("root")).await;
        let state = AppState::for_test(pool.clone());
        let _daemon = fake_daemon(&state, machine);
        let (_, axum::Json(res)) = crate::routes::sessions::fork_session(
            State(state),
            user_ctx(uid),
            Path(sid.clone()),
            axum::Json(cctui_proto::api::ForkRequest::default()),
        )
        .await
        .expect("fork");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_FORKED]);
        assert_eq!(rows[0].detail["child_session_id"], res.session_id.expect("claude pre-mints"));
        assert_eq!(rows[0].detail["parent_session_id"], sid);
        assert!(rows[0].summary.starts_with("root forked into "));
    }

    #[tokio::test]
    async fn resume_records_exactly_one_manual_row() {
        use axum::extract::{Path, State};
        let Some(pool) = test_pool("events_resume_once").await else { return };
        let (uid, machine) = seed_user_machine(&pool, "ev-resume").await;
        let sid = seed_session(&pool, uid, machine, Some("paused")).await;
        sqlx::query("UPDATE sessions SET status = 'ended', end_reason = 'completed' WHERE id = $1")
            .bind(&sid)
            .execute(&pool)
            .await
            .unwrap();
        let state = AppState::for_test(pool.clone());
        let _daemon = fake_daemon(&state, machine);
        crate::routes::sessions::resume_session(State(state), user_ctx(uid), Path(sid.clone()))
            .await
            .expect("resume");
        settle().await;
        let rows = rows_for_session(&pool, &sid).await;
        assert_eq!(kinds(&rows), vec![kind::SESSION_RESUMED]);
        assert_eq!(rows[0].detail["origin"], "manual");
        assert_eq!(rows[0].summary, "paused resumed (manual)");
    }

    #[tokio::test]
    async fn retention_prunes_only_past_the_window_and_zero_keeps_everything() {
        let Some(pool) = test_pool("events_retention").await else { return };
        let marker = format!("retention-{}", Uuid::new_v4());
        let seed = |age: &'static str| {
            let pool = pool.clone();
            let marker = marker.clone();
            async move {
                sqlx::query(
                    "INSERT INTO events (occurred_at, kind, actor, summary, detail) \
                     VALUES (now() - $1::interval, 'system.test', 'system', $2, '{}')",
                )
                .bind(age)
                .bind(marker)
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        seed("100 days").await;
        seed("1 day").await;
        let count = || {
            let pool = pool.clone();
            let marker = marker.clone();
            async move {
                sqlx::query_scalar::<_, i64>("SELECT count(*) FROM events WHERE summary = $1")
                    .bind(marker)
                    .fetch_one(&pool)
                    .await
                    .unwrap()
            }
        };
        let set = |days: i64| {
            let pool = pool.clone();
            async move {
                sqlx::query(
                    "INSERT INTO instance_settings (key, value, updated_at) VALUES ($1, $2, now()) \
                     ON CONFLICT (key) DO UPDATE SET value = EXCLUDED.value",
                )
                .bind(RETENTION_KEY)
                .bind(json!(days))
                .execute(&pool)
                .await
                .unwrap();
            }
        };
        set(0).await;
        assert_eq!(retention_days(&pool).await, 0);
        prune(&pool).await;
        assert_eq!(count().await, 2);
        set(90).await;
        prune(&pool).await;
        assert_eq!(count().await, 1);
        sqlx::query("DELETE FROM instance_settings WHERE key = $1")
            .bind(RETENTION_KEY)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(retention_days(&pool).await, DEFAULT_RETENTION_DAYS);
    }
}
