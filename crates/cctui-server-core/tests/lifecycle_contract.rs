//! Essential lifecycle storage never needs the workspace archive.
use cctui_proto::adapter::EndReason;
use cctui_server_core::bus::{Bus, NoopTransport};
use cctui_server_core::commands::FailedSpawnRow;
use cctui_server_core::events::{Actor, Event, kind, record_now};
use cctui_server_core::lifecycle::{
    NoopSessionEndJournal, SessionEndJournal, persist_failed_spawn, persist_session_end,
    upsert_session,
};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

struct Fixture {
    pool: PgPool,
    bus: Bus,
    machine: Uuid,
    user: Uuid,
}
impl Fixture {
    async fn new() -> Option<Self> {
        let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
            .iter()
            .filter_map(|k| std::env::var(k).ok())
            .find(|v| !v.trim().is_empty());
        let Some(url) = url else {
            assert!(std::env::var_os("CI").is_none(), "lifecycle contracts need a database in CI");
            eprintln!("skipping lifecycle contracts: no test database");
            return None;
        };
        let pool = sqlx::postgres::PgPoolOptions::new()
            .max_connections(1)
            .after_connect(|conn, _| {
                Box::pin(async move {
                    sqlx::query("SET search_path = pg_temp").execute(conn).await?;
                    Ok(())
                })
            })
            .connect(&url)
            .await
            .unwrap();
        for ddl in [
            "CREATE TEMP TABLE sessions (LIKE public.sessions INCLUDING ALL)",
            "CREATE TEMP TABLE session_tokens (LIKE public.session_tokens INCLUDING ALL)",
            "CREATE TEMP TABLE machines (LIKE public.machines INCLUDING ALL)",
            "CREATE TEMP TABLE events (LIKE public.events INCLUDING ALL)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        assert!(
            sqlx::query_scalar::<_, Option<String>>("SELECT to_regclass('stream_events')::text")
                .fetch_one(&pool)
                .await
                .unwrap()
                .is_none()
        );
        Some(Self {
            pool,
            bus: Bus::new(Box::new(NoopTransport)),
            machine: Uuid::new_v4(),
            user: Uuid::new_v4(),
        })
    }
    async fn register(&self, id: &str) -> Option<bool> {
        upsert_session(
            &self.pool,
            &self.bus,
            self.machine,
            self.user,
            "codex",
            id,
            Some("/tmp/review".into()),
            None,
            Some(1_700_000_000),
            Some(json!({"relation":"followup"})),
        )
        .await
        .unwrap()
    }
    async fn status(&self, id: &str) -> String {
        sqlx::query_scalar("SELECT status FROM sessions WHERE id = $1")
            .bind(id)
            .fetch_one(&self.pool)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn registration_preserves_ownership_relationships_and_preexisting_account_binding() {
    let Some(f) = Fixture::new().await else { return };
    let session = Uuid::new_v4().to_string();
    let provider = Uuid::new_v4();
    sqlx::query("INSERT INTO session_tokens (session_id, account_id, token_hash) VALUES ($1, $2, 'fixture-hash')")
        .bind(&session).bind(provider).execute(&f.pool).await.unwrap();
    assert_eq!(f.register(&session).await, Some(true));
    assert_eq!(f.register(&session).await, Some(false));
    let account: Option<String> =
        sqlx::query_scalar("SELECT account_id FROM sessions WHERE id = $1")
            .bind(&session)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(account, Some(provider.to_string()));
    let owner_before: (Option<Uuid>, Option<Uuid>, serde_json::Value) =
        sqlx::query_as("SELECT machine_uuid, user_id, metadata FROM sessions WHERE id = $1")
            .bind(&session)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let foreign = upsert_session(
        &f.pool,
        &f.bus,
        Uuid::new_v4(),
        f.user,
        "codex",
        &session,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(foreign, None);
    upsert_session(
        &f.pool,
        &f.bus,
        f.machine,
        f.user,
        "codex",
        &session,
        None,
        None,
        None,
        Some(json!({"relation":"root"})),
    )
    .await
    .unwrap();
    let after: (Option<Uuid>, Option<Uuid>, serde_json::Value) =
        sqlx::query_as("SELECT machine_uuid, user_id, metadata FROM sessions WHERE id = $1")
            .bind(&session)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(after, owner_before);
}

#[tokio::test]
async fn terminal_state_and_failed_spawn_need_no_transcript_archive() {
    let Some(f) = Fixture::new().await else { return };
    let session = Uuid::new_v4().to_string();
    f.register(&session).await;
    let reason = EndReason::SpawnFailed { detail: "失敗".repeat(1000) };
    assert!(
        persist_session_end(&f.pool, &NoopSessionEndJournal, f.machine, f.user, &session, &reason)
            .await
            .unwrap()
    );
    let (status, detail): (String, String) =
        sqlx::query_as("SELECT status, end_detail FROM sessions WHERE id = $1")
            .bind(&session)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(status, "ended");
    assert!(detail.len() <= 2048 && reason.detail().unwrap().starts_with(&detail));
    assert!(
        !persist_session_end(
            &f.pool,
            &NoopSessionEndJournal,
            Uuid::new_v4(),
            f.user,
            &session,
            &EndReason::Killed
        )
        .await
        .unwrap()
    );
    let failed = FailedSpawnRow {
        session_id: Uuid::new_v4().to_string(),
        machine_id: f.machine,
        user_id: f.user,
        adapter_id: "codex".into(),
        working_dir: "/tmp/review".into(),
        name: None,
        model: None,
        effort: None,
    };
    assert!(persist_failed_spawn(&f.pool, &NoopSessionEndJournal, &failed, &reason).await.unwrap());
    assert_eq!(f.status(&failed.session_id).await, "ended");
    assert!(
        !persist_failed_spawn(&f.pool, &NoopSessionEndJournal, &failed, &EndReason::Killed)
            .await
            .unwrap()
    );
}

struct FailingJournal;
impl SessionEndJournal for FailingJournal {
    async fn before_end(
        &self,
        _pool: &PgPool,
        _machine_id: Uuid,
        _user_id: Uuid,
        _local_id: &str,
        _reason: &EndReason,
    ) -> Result<(), sqlx::Error> {
        Err(sqlx::Error::RowNotFound)
    }
}

#[tokio::test]
async fn a_failed_journal_leaves_the_owned_session_status_unchanged() {
    let Some(f) = Fixture::new().await else { return };
    let session = Uuid::new_v4().to_string();
    f.register(&session).await;
    assert!(
        persist_session_end(
            &f.pool,
            &FailingJournal,
            f.machine,
            f.user,
            &session,
            &EndReason::Killed
        )
        .await
        .is_err()
    );
    assert_eq!(f.status(&session).await, "active");
}

#[tokio::test]
async fn spurious_offline_ends_revive_but_real_crashes_stay_terminal() {
    let Some(f) = Fixture::new().await else { return };
    let mut frames = f.bus.subscribe_server();
    for (reason, expected) in [("machine_offline", "active"), ("crashed", "ended")] {
        let session = Uuid::new_v4().to_string();
        f.register(&session).await;
        sqlx::query(
            "UPDATE sessions SET status = 'ended', end_reason = $2, ended_at = now() WHERE id = $1",
        )
        .bind(&session)
        .bind(reason)
        .execute(&f.pool)
        .await
        .unwrap();
        f.register(&session).await;
        assert_eq!(f.status(&session).await, expected);
        if reason == "machine_offline" {
            let event = tokio::time::timeout(std::time::Duration::from_secs(2), frames.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(event.json.contains(kind::SESSION_RESUMED));
        }
    }
}

#[tokio::test]
async fn audit_record_is_published_only_after_it_is_persisted() {
    let Some(f) = Fixture::new().await else { return };
    let session = Uuid::new_v4().to_string();
    f.register(&session).await;
    let mut frames = f.bus.subscribe_server();
    let event = record_now(
        &f.pool,
        &f.bus,
        Event::new(kind::SESSION_CREATED, Actor::Daemon).session(&session),
    )
    .await
    .unwrap();
    let stored: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM events WHERE id = $1)")
        .bind(event.id)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert!(stored);
    let published: serde_json::Value =
        serde_json::from_str(&frames.try_recv().unwrap().json).unwrap();
    assert_eq!(published["event"]["id"], event.id);
    assert_eq!(event.user_id, Some(f.user));
}
