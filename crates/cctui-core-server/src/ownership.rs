//! The standalone host's single-active-process boundary.
use anyhow::Context;
use sqlx::{Connection, PgConnection, PgPool};
use std::time::Duration;

// Separate from the schema migration lock; scoped by PostgreSQL to this database.
const LOCK_KEY: i64 = 0x6363_7475_695f_7274;

pub struct RuntimeLease(PgConnection);

impl RuntimeLease {
    pub async fn acquire(pool: &PgPool) -> anyhow::Result<Self> {
        // Never return a connection holding a session lock to the shared pool.
        let mut conn = pool.acquire().await?.detach();
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1)")
            .bind(LOCK_KEY)
            .fetch_one(&mut conn)
            .await?;
        anyhow::ensure!(
            acquired,
            "another standalone core owns this database; stop it before starting a replacement"
        );
        Ok(Self(conn))
    }

    pub async fn monitor(mut self) -> anyhow::Result<()> {
        let mut tick = tokio::time::interval(Duration::from_secs(5));
        loop {
            tick.tick().await;
            tokio::time::timeout(Duration::from_secs(5), self.0.ping())
                .await
                .context("standalone core execution ownership connection lost: probe timed out")?
                .context("standalone core execution ownership connection lost")?;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cctui_server_core::test_support::with_isolated_database;

    #[tokio::test]
    async fn second_host_is_denied_until_the_owner_exits() {
        with_isolated_database("core-single-owner", |pool| async move {
            let first = RuntimeLease::acquire(&pool).await.unwrap();
            let second = RuntimeLease::acquire(&pool).await;
            assert!(second.is_err(), "two standalone hosts cannot own one database");
            drop(first);
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let Ok(replacement) = RuntimeLease::acquire(&pool).await {
                        return replacement;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("exiting the owner must release its physical connection");
        })
        .await;
    }

    #[tokio::test]
    async fn ownership_connection_loss_is_fatal_and_releases_the_database() {
        with_isolated_database("core-lost-owner", |pool| async move {
            let mut lease = RuntimeLease::acquire(&pool).await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut lease.0)
                .await
                .unwrap();
            let monitor = tokio::spawn(lease.monitor());
            assert!(RuntimeLease::acquire(&pool).await.is_err());
            let terminated: bool = sqlx::query_scalar("SELECT pg_terminate_backend($1)")
                .bind(pid)
                .fetch_one(&pool)
                .await
                .unwrap();
            assert!(terminated);
            let failure = tokio::time::timeout(Duration::from_secs(8), monitor)
                .await
                .expect("ownership loss must stop the host")
                .unwrap()
                .expect_err("a lost connection cannot leave execution running");
            assert!(failure.to_string().contains("ownership connection lost"));
            RuntimeLease::acquire(&pool).await.unwrap();
        })
        .await;
    }
}
