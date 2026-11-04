//! Workspace provisioning without changing the legacy migration ledger.
use cctui_server_core::schema::{Migration, MigrationConnection, Origin};
use sqlx::PgConnection;

/// The workspace baseline corresponding to legacy schema 191.
pub const BOOTSTRAP: &str = include_str!("../schema/0001_workspace.sql");

pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, sql: BOOTSTRAP },
    Migration { version: 2, sql: include_str!("../schema/0002_dispatch_identities.sql") },
    Migration {
        version: 3,
        sql: include_str!("../schema/0003_scheduled_message_sealed_body.sql"),
    },
];

/// Expand a core installation, or adopt the already migrated full schema.
pub async fn provision(conn: &mut MigrationConnection) -> Result<(), sqlx::Error> {
    let adopted = cctui_server_core::schema::origin(conn).await? == Some(Origin::LegacyFull);
    if core_created_dispatch_identities(conn).await? {
        cctui_server_core::schema::install_package(conn, "workspace", &MIGRATIONS[..1], adopted)
            .await?;
        cctui_server_core::schema::adopt(conn, "workspace", MIGRATIONS[1]).await?;
    }
    cctui_server_core::schema::install_package(conn, "workspace", MIGRATIONS, adopted).await?;
    Ok(())
}

/// Core migration five creates the table workspace migration two would create again.
async fn core_created_dispatch_identities(conn: &mut PgConnection) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT to_regclass('public.dispatch_identities') IS NOT NULL \
         AND (SELECT count(*) FROM _cctui_schema_migrations WHERE package = 'workspace') < 2",
    )
    .fetch_one(conn)
    .await
}

#[cfg(test)]
mod tests {
    use cctui_server_core::test_support::with_isolated_database;

    async fn provisioned(pool: &sqlx::PgPool) {
        cctui_server_core::schema::bootstrap(pool).await.unwrap();
        let mut conn = cctui_server_core::schema::migration_connection(pool).await.unwrap();
        super::provision(&mut conn).await.unwrap();
    }

    async fn installations(pool: &sqlx::PgPool, package: &str) -> Vec<(i64, String)> {
        sqlx::query_as(
            "SELECT version, installation FROM _cctui_schema_migrations WHERE package = $1 \
             ORDER BY version",
        )
        .bind(package)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    async fn insert_identity(pool: &sqlx::PgPool, id: uuid::Uuid) -> u64 {
        sqlx::query(
            "INSERT INTO dispatch_identities (dispatch_id, dispatcher_id, user_id, \
             session_id, namespace, job_name, machine_key_hash, expires_at) \
             VALUES ($1, $1, $1, 's', 'ns', 'job', 'h', now())",
        )
        .bind(id)
        .execute(pool)
        .await
        .unwrap()
        .rows_affected()
    }

    #[tokio::test]
    async fn fresh_full_provisioning_adopts_the_core_dispatch_identity_table() {
        with_isolated_database("dispatch_identity_migration", |pool| async move {
            provisioned(&pool).await;
            let present: bool =
                sqlx::query_scalar("SELECT to_regclass('public.dispatch_identities') IS NOT NULL")
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert!(present);
            let mut conn = cctui_server_core::schema::migration_connection(&pool).await.unwrap();
            super::provision(&mut conn).await.unwrap();
            drop(conn);
            assert_eq!(
                installations(&pool, "workspace").await,
                [
                    (1, "bootstrap".to_owned()),
                    (2, "adopted".to_owned()),
                    (3, "bootstrap".to_owned())
                ]
            );

            sqlx::raw_sql("DROP TABLE public.dispatch_identities").execute(&pool).await.unwrap();
            sqlx::raw_sql(super::MIGRATIONS[1].sql).execute(&pool).await.unwrap();
            let inserted = insert_identity(&pool, uuid::Uuid::new_v4()).await;
            assert_eq!(inserted, 1, "re-applied up works");
        })
        .await;
    }

    #[tokio::test]
    async fn a_database_with_workspace_two_installed_takes_core_five_cleanly() {
        with_isolated_database("dispatch_identity_preexisting", |pool| async move {
            provisioned(&pool).await;
            sqlx::raw_sql(
                "DELETE FROM _cctui_schema_migrations WHERE package = 'core' AND version >= 5; \
                 UPDATE _cctui_schema_migrations SET installation = 'bootstrap' \
                 WHERE package = 'workspace' AND version = 2",
            )
            .execute(&pool)
            .await
            .unwrap();
            let kept = uuid::Uuid::new_v4();
            assert_eq!(insert_identity(&pool, kept).await, 1);

            provisioned(&pool).await;
            let core: Vec<i64> =
                installations(&pool, "core").await.into_iter().map(|(v, _)| v).collect();
            assert_eq!(core, [1, 2, 3, 4, 5, 6]);
            assert_eq!(
                installations(&pool, "workspace").await,
                [
                    (1, "bootstrap".to_owned()),
                    (2, "bootstrap".to_owned()),
                    (3, "bootstrap".to_owned())
                ]
            );
            let rows: i64 =
                sqlx::query_scalar("SELECT count(*) FROM dispatch_identities WHERE dispatch_id=$1")
                    .bind(kept)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
            assert_eq!(rows, 1);
        })
        .await;
    }
}
