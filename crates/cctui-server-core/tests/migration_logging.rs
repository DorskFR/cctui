//! Migration progress logs, in a binary of their own: tracing caches whether a
//! log line is enabled process-wide, so a capturing subscriber cannot share a
//! process with tests running in parallel.
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use cctui_server_core::schema::{Migration, bootstrap, install_package, migration_connection};
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Captured {
    fn lines(&self) -> Vec<String> {
        let text = String::from_utf8(std::mem::take(&mut *self.0.lock().unwrap())).unwrap();
        text.lines().filter(|l| l.contains("migration")).map(str::to_owned).collect()
    }
}

#[tokio::test]
async fn each_pending_migration_logs_its_start_and_completion_before_the_summary() {
    let Some(url) = ["DATABASE_URL", "TEST_DATABASE_URL"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
    else {
        assert!(std::env::var_os("CI").is_none(), "DATABASE_URL must be set in CI");
        return;
    };
    let captured = Captured::default();
    let writer = captured.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();

    let options = PgConnectOptions::from_str(&url).unwrap();
    let admin =
        PgPoolOptions::new().max_connections(1).connect_with(options.clone()).await.unwrap();
    let database = format!("cctui_schema_{}", uuid::Uuid::new_v4().simple());
    sqlx::query(sqlx::AssertSqlSafe(format!("CREATE DATABASE {database}")))
        .execute(&admin)
        .await
        .unwrap();
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect_with(options.database(&database))
        .await
        .unwrap();

    bootstrap(&pool).await.unwrap();
    captured.lines();
    let migrations = [
        Migration { version: 1, name: "first", sql: "CREATE TABLE fixture_logged (id bigint);" },
        Migration { version: 2, name: "second", sql: "SELECT pg_sleep(0.05);" },
    ];
    let mut conn = migration_connection(&pool).await.unwrap();
    install_package(&mut conn, "fixture", &migrations, false).await.unwrap();
    let first_run = captured.lines();
    install_package(&mut conn, "fixture", &migrations, false).await.unwrap();
    let second_run = captured.lines();

    drop(conn);
    pool.close().await;
    sqlx::query(sqlx::AssertSqlSafe(format!("DROP DATABASE {database} WITH (FORCE)")))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;

    let expected = [
        "applying migration 1 first; this can take a while on large tables",
        "applied migration 1 first in ",
        "applying migration 2 second; this can take a while on large tables",
        "applied migration 2 second in ",
        "applied 2 fixture migrations in ",
    ];
    assert_eq!(first_run.len(), expected.len(), "{first_run:#?}");
    for (line, message) in first_run.iter().zip(expected) {
        assert!(line.contains(" INFO ") && line.contains(message), "{message:?} in {line:?}");
        assert!(line.contains("package=\"fixture\""), "{line}");
    }
    assert!(first_run[3].contains("elapsed="), "{}", first_run[3]);
    assert!(second_run.is_empty(), "an up-to-date package logs nothing: {second_run:#?}");
}
