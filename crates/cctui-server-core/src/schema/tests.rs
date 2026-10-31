use super::*;
use crate::machine_launches as launches;
use crate::test_support::with_isolated_database;

#[tokio::test]
async fn concurrent_core_bootstraps_create_only_execution_storage_without_extensions() {
    with_isolated_database("concurrent_core_bootstrap", |pool| async move {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (a, b) = tokio::join!(bootstrap(&pool), bootstrap(&pool));
            a.unwrap();
            b.unwrap();
        })
        .await
        .unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT count(*) FROM pg_tables WHERE schemaname='public'")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 37, "35 execution tables plus two schema tracking tables");
        let extensions: Vec<String> =
            sqlx::query_scalar("SELECT extname FROM pg_extension ORDER BY extname")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(extensions, ["plpgsql"]);
        let legacy: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(!legacy, "core must never manufacture legacy migration history");
        let entries: i64 = sqlx::query_scalar("SELECT count(*) FROM _cctui_schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(entries, 4);
    })
    .await;
}

#[tokio::test]
async fn changed_checksums_and_future_core_versions_are_rejected() {
    with_isolated_database("core_schema_versions", |pool| async move {
        bootstrap(&pool).await.unwrap();
        sqlx::query("UPDATE _cctui_schema_migrations SET checksum='\\x00'::bytea")
            .execute(&pool)
            .await
            .unwrap();
        assert!(bootstrap(&pool).await.is_err());
        sqlx::query("UPDATE _cctui_schema_migrations SET checksum=$1,version=version+100")
            .bind(Sha256::digest(BOOTSTRAP.as_bytes()).to_vec())
            .execute(&pool)
            .await
            .unwrap();
        assert!(bootstrap(&pool).await.is_err());
    })
    .await;
}

#[tokio::test]
async fn a_failed_package_rolls_back_its_schema_and_tracking() {
    with_isolated_database("package_rollback", |pool| async move {
        bootstrap(&pool).await.unwrap();
        let mut conn = migration_connection(&pool).await.unwrap();
        let failed = install_package(
            &mut conn,
            "fixture",
            &[Migration {
                version: 1,
                sql: "CREATE TABLE fixture_partial (id bigint); SELECT 1/0;",
            }],
            false,
        )
        .await;
        assert!(failed.is_err());
        let absent: bool = sqlx::query_scalar(
            "SELECT to_regclass('fixture_partial') IS NULL AND NOT EXISTS ( \
                SELECT 1 FROM _cctui_schema_migrations WHERE package='fixture')",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(absent);
        provision(&mut conn).await.unwrap();
    })
    .await;
}

#[tokio::test]
async fn canceling_a_lock_waiter_does_not_leak_a_migration_lock() {
    with_isolated_database("migration_lock_cancellation", |pool| async move {
        let holder = migration_connection(&pool).await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(30), bootstrap(&pool)).await.is_err());
        drop(holder);
        tokio::time::timeout(Duration::from_secs(3), bootstrap(&pool)).await.unwrap().unwrap();
    })
    .await;
}

#[tokio::test]
async fn an_unrecognized_schema_is_preserved_and_refused() {
    with_isolated_database("unknown_core_schema", |pool| async move {
        sqlx::raw_sql("CREATE TABLE application_data (value text); INSERT INTO application_data VALUES ('keep');")
            .execute(&pool).await.unwrap();
        assert!(bootstrap(&pool).await.is_err());
        let value: String = sqlx::query_scalar("SELECT value FROM application_data")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(value, "keep");
        let untouched: bool = sqlx::query_scalar("SELECT to_regclass('_cctui_schema_lineage') IS NULL")
            .fetch_one(&pool).await.unwrap();
        assert!(untouched);
    })
    .await;
}

#[tokio::test]
async fn core_forward_migration_adds_durable_machine_launch_storage() {
    with_isolated_database("core_forward_launch_storage", |pool| async move {
        bootstrap(&pool).await.unwrap();
        let exists: bool = sqlx::query_scalar("SELECT to_regclass('machine_launches') IS NOT NULL")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(exists, "accepted machine launches need durable identity before registration");
    })
    .await;
}

#[tokio::test]
async fn a_baseline_only_core_upgrades_without_changing_existing_data_or_history() {
    with_isolated_database("core_forward_upgrade", |pool| async move {
        bootstrap(&pool).await.unwrap();
        sqlx::raw_sql("DROP TABLE machine_launches,execution_stops; DELETE FROM _cctui_schema_migrations WHERE package='core' AND version>=2; INSERT INTO users (id,name,key_hash) VALUES ('00000000-0000-0000-0000-000000000001','fixture','fixture-hash');")
            .execute(&pool).await.unwrap();
        let baseline: (Vec<u8>,String) = sqlx::query_as("SELECT checksum,installation FROM _cctui_schema_migrations WHERE package='core' AND version=1")
            .fetch_one(&pool).await.unwrap();
        bootstrap(&pool).await.unwrap();
        bootstrap(&pool).await.unwrap();
        let after: (Vec<u8>,String) = sqlx::query_as("SELECT checksum,installation FROM _cctui_schema_migrations WHERE package='core' AND version=1")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(after,baseline);
        let name: String=sqlx::query_scalar("SELECT name FROM users WHERE key_hash='fixture-hash'").fetch_one(&pool).await.unwrap();
        assert_eq!(name,"fixture");
        let versions: Vec<i64>=sqlx::query_scalar("SELECT version FROM _cctui_schema_migrations WHERE package='core' ORDER BY version").fetch_all(&pool).await.unwrap();
        assert_eq!(versions,[1,2,3,4]);
    }).await;
}

#[tokio::test]
async fn failed_forward_sql_keeps_prior_migrations_and_can_resume() {
    with_isolated_database("package_forward_rollback", |pool| async move {
        bootstrap(&pool).await.unwrap();
        let mut conn=migration_connection(&pool).await.unwrap();
        let baseline=Migration{version:1,sql:"CREATE TABLE fixture_forward (value text); INSERT INTO fixture_forward VALUES ('keep');"};
        let failed=Migration{version:2,sql:"ALTER TABLE fixture_forward ADD COLUMN partial bigint; SELECT 1/0;"};
        assert!(install_package(&mut conn,"fixture",&[baseline,failed],false).await.is_err());
        let absent:bool=sqlx::query_scalar("SELECT NOT EXISTS(SELECT 1 FROM information_schema.columns WHERE table_name='fixture_forward' AND column_name='partial')").fetch_one(&mut *conn).await.unwrap();
        assert!(absent);
        let count:i64=sqlx::query_scalar("SELECT count(*) FROM _cctui_schema_migrations WHERE package='fixture'").fetch_one(&mut *conn).await.unwrap();
        assert_eq!(count,1);
        let fixed=Migration{version:2,sql:"ALTER TABLE fixture_forward ADD COLUMN finished bigint;"};
        install_package(&mut conn,"fixture",&[baseline,fixed],false).await.unwrap();
        install_package(&mut conn,"fixture",&[baseline,fixed],false).await.unwrap();
        let value:String=sqlx::query_scalar("SELECT value FROM fixture_forward").fetch_one(&mut *conn).await.unwrap();
        assert_eq!(value,"keep");
        assert!(install_package(&mut conn,"fixture",&[baseline],false).await.is_err(),"unknown newer versions are refused");
    }).await;
}

#[tokio::test]
async fn durable_launches_bind_verdicts_aliases_and_results_to_the_authorized_identities() {
    with_isolated_database("durable_launch_ownership", |pool| async move {
        bootstrap(&pool).await.unwrap();
        let owner = uuid::Uuid::new_v4();
        let actor = uuid::Uuid::new_v4();
        let machine = uuid::Uuid::new_v4();
        for id in [owner, actor] {
            sqlx::query("INSERT INTO users (id,name,key_hash) VALUES ($1,$2,$2)")
                .bind(id)
                .bind(id.to_string())
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO machines (id,user_id,name,key_hash) VALUES ($1,$2,$3,$3)")
            .bind(machine)
            .bind(owner)
            .bind(machine.to_string())
            .execute(&pool)
            .await
            .unwrap();
        let command = uuid::Uuid::new_v4();
        let key = command.to_string();
        let row = crate::commands::FailedSpawnRow {
            session_id: key.clone(),
            machine_id: machine,
            user_id: owner,
            adapter_id: "codex".into(),
            working_dir: "/work/acme".into(),
            name: None,
            model: None,
            effort: None,
        };
        launches::prepare(&pool, command, actor, &row).await.unwrap();
        assert!(
            launches::verdict(&pool, command, uuid::Uuid::new_v4(), owner, false, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            launches::verdict(&pool, command, machine, actor, false, None).await.unwrap().is_none()
        );
        assert!(launches::find(&pool, &key, Some(owner)).await.unwrap().is_none());
        assert_eq!(
            launches::find(&pool, &key, Some(actor)).await.unwrap().unwrap().state,
            "prepared"
        );
        launches::registered(&pool, &key, "foreign", machine, actor).await.unwrap();
        assert!(launches::find(&pool, "foreign", None).await.unwrap().is_none());
        launches::registered(&pool, &key, "canonical", machine, owner).await.unwrap();
        assert_eq!(
            launches::find(&pool, &key, Some(actor))
                .await
                .unwrap()
                .unwrap()
                .registered_session_id
                .as_deref(),
            Some("canonical")
        );
        assert!(launches::find(&pool, "canonical", Some(owner)).await.unwrap().is_none());
        assert!(launches::find(&pool, "canonical", Some(actor)).await.unwrap().is_some());
        assert!(
            launches::verdict(&pool, command, machine, owner, false, Some("late refusal"))
                .await
                .unwrap()
                .is_none()
        );
        launches::delivered(&pool, command).await.unwrap();
        launches::delivery_failed(&pool, command, "late transport failure").await.unwrap();
        let launch = launches::find(&pool, &key, None).await.unwrap().unwrap();
        assert_eq!(launch.state, "registered");
        assert_eq!(launch.registered_session_id.as_deref(), Some("canonical"));
    })
    .await;
}
