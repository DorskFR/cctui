use super::*;
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
        assert_eq!(count, 35, "33 execution tables plus two schema tracking tables");
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
        assert_eq!(entries, 1);
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
        sqlx::query("UPDATE _cctui_schema_migrations SET checksum=$1,version=2")
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
            "CREATE TABLE fixture_partial (id bigint); SELECT 1/0;",
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
