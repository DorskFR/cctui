//! Delivery-level idempotence for replies, keyed by the client's `turn_id`.
//!
//! A reply is not idempotent and a client that never saw its ack resends the
//! same turn, so without a record the agent is handed one copy per attempt.
//! The record is a DB row, not a process map: replicas have no affinity, and
//! the resend that matters most follows a reconnect through the ingress.
//!
//! A claim is a reservation, not a receipt. It is taken before dispatch so two
//! attempts cannot race into the agent, and it only *sticks* once the dispatch
//! succeeded — a refused one is released, because the client's retry of a
//! message the daemon never took has to reach the agent.

use std::time::Duration;

use sqlx::PgPool;
use uuid::Uuid;

/// How long an unconfirmed claim blocks a retry of its own turn. A replica that
/// dies between claiming and dispatching leaves one behind; past this it is
/// reclaimable, so a crash cannot wedge a session's turn forever.
pub const IN_FLIGHT_TTL: Duration = Duration::from_secs(30);

/// How long a confirmed claim is kept. Comfortably longer than any client's
/// retry ladder, short enough that the table stays small.
pub const CLAIM_TTL: Duration = Duration::from_hours(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Claim {
    /// The caller owns this turn and must dispatch it, then confirm or release.
    Granted,
    /// A previous attempt already reached the agent, under this command.
    AlreadyDispatched { command_id: Uuid },
    /// Another attempt is dispatching right now. Not an error and not a
    /// delivery: the caller must report it as undelivered so the client retries.
    InFlight,
}

/// Reserves `turn_id` for `session_id`.
pub async fn claim(pool: &PgPool, session_id: &str, turn_id: Uuid) -> Result<Claim, sqlx::Error> {
    let inserted = sqlx::query(
        "INSERT INTO dispatched_turns (session_id, turn_id) VALUES ($1, $2) \
         ON CONFLICT (session_id, turn_id) DO NOTHING",
    )
    .bind(session_id)
    .bind(turn_id)
    .execute(pool)
    .await?;
    if inserted.rows_affected() == 1 {
        return Ok(Claim::Granted);
    }

    let existing: Option<(Option<Uuid>, bool)> = sqlx::query_as(
        "SELECT command_id, claimed_at < now() - $3::interval AS stale \
         FROM dispatched_turns WHERE session_id = $1 AND turn_id = $2",
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(IN_FLIGHT_TTL)
    .fetch_optional(pool)
    .await?;

    match existing {
        Some((Some(command_id), _)) => Ok(Claim::AlreadyDispatched { command_id }),
        Some((None, true)) => take_over(pool, session_id, turn_id).await,
        // Still dispatching, or the row was released between the two statements
        // — either way this attempt did not deliver anything.
        Some((None, false)) | None => Ok(Claim::InFlight),
    }
}

/// Steals an abandoned claim. The `command_id IS NULL` guard is what makes the
/// steal atomic: only one caller can win it.
async fn take_over(pool: &PgPool, session_id: &str, turn_id: Uuid) -> Result<Claim, sqlx::Error> {
    let stolen = sqlx::query(
        "UPDATE dispatched_turns SET claimed_at = now() \
         WHERE session_id = $1 AND turn_id = $2 AND command_id IS NULL \
           AND claimed_at < now() - $3::interval",
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(IN_FLIGHT_TTL)
    .execute(pool)
    .await?;
    Ok(if stolen.rows_affected() == 1 { Claim::Granted } else { Claim::InFlight })
}

/// Marks the claim as actually delivered, so later retries are deduped against
/// a command the client can still resolve a delivery state from.
pub async fn confirm(
    pool: &PgPool,
    session_id: &str,
    turn_id: Uuid,
    command_id: Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE dispatched_turns SET command_id = $3, claimed_at = now() \
         WHERE session_id = $1 AND turn_id = $2",
    )
    .bind(session_id)
    .bind(turn_id)
    .bind(command_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Drops a claim whose dispatch was refused, so the client's retry is dispatched
/// rather than acked as a duplicate of something that never ran.
pub async fn release(pool: &PgPool, session_id: &str, turn_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM dispatched_turns \
         WHERE session_id = $1 AND turn_id = $2 AND command_id IS NULL",
    )
    .bind(session_id)
    .bind(turn_id)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Prunes claims past their keep window.
pub async fn sweep(pool: &PgPool) {
    let deleted =
        sqlx::query("DELETE FROM dispatched_turns WHERE claimed_at < now() - $1::interval")
            .bind(CLAIM_TTL)
            .execute(pool)
            .await;
    match deleted {
        Ok(done) if done.rows_affected() > 0 => {
            tracing::debug!(rows = done.rows_affected(), "swept dispatched turns");
        }
        Ok(_) => {}
        Err(e) => tracing::warn!(%e, "sweeping dispatched turns failed"),
    }
}

#[cfg(test)]
mod tests {
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{Claim, claim, confirm, release, sweep};

    async fn seed(pool: &PgPool, name: &str) -> String {
        let id = format!("{name}-{}", Uuid::new_v4());
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, working_dir, status) \
             VALUES ($1, $2, '/tmp', 'active')",
        )
        .bind(&id)
        .bind(&id)
        .execute(pool)
        .await
        .expect("seed session");
        id
    }

    async fn pool_for(name: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(4)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    /// R1: the claim must not stick when the daemon refused the dispatch, or the
    /// retry is acked as a duplicate of a message that never ran.
    #[tokio::test]
    async fn a_released_claim_lets_the_retry_dispatch_again() {
        let name = "turn_dedupe_release";
        let Some(pool) = pool_for(name).await else { return };
        let session = seed(&pool, name).await;
        let turn = Uuid::new_v4();

        assert_eq!(claim(&pool, &session, turn).await.unwrap(), Claim::Granted);
        // The daemon was offline: the attempt delivered nothing.
        release(&pool, &session, turn).await.unwrap();

        assert_eq!(
            claim(&pool, &session, turn).await.unwrap(),
            Claim::Granted,
            "a refused turn must be dispatchable again"
        );

        let command = Uuid::new_v4();
        confirm(&pool, &session, turn, command).await.unwrap();
        assert_eq!(
            claim(&pool, &session, turn).await.unwrap(),
            Claim::AlreadyDispatched { command_id: command },
            "once it really went out, a resend is a duplicate"
        );
        // A confirmed claim is not released by a late refusal of another attempt.
        release(&pool, &session, turn).await.unwrap();
        assert_eq!(
            claim(&pool, &session, turn).await.unwrap(),
            Claim::AlreadyDispatched { command_id: command }
        );
    }

    /// R2: two replicas share no process state, only this table.
    #[tokio::test]
    async fn a_second_replica_sees_the_claim_and_does_not_dispatch() {
        let name = "turn_dedupe_replicas";
        let Some(replica_a) = pool_for(name).await else { return };
        let Some(replica_b) = pool_for(name).await else { return };
        let session = seed(&replica_a, name).await;
        let turn = Uuid::new_v4();
        let command = Uuid::new_v4();

        assert_eq!(claim(&replica_a, &session, turn).await.unwrap(), Claim::Granted);
        confirm(&replica_a, &session, turn, command).await.unwrap();

        assert_eq!(
            claim(&replica_b, &session, turn).await.unwrap(),
            Claim::AlreadyDispatched { command_id: command },
            "a reconnect to the other replica must not deliver a second copy"
        );
    }

    /// Concurrent attempts on different replicas: exactly one dispatches.
    #[tokio::test]
    async fn only_one_of_two_racing_replicas_is_granted() {
        let name = "turn_dedupe_race";
        let Some(replica_a) = pool_for(name).await else { return };
        let Some(replica_b) = pool_for(name).await else { return };
        let session = seed(&replica_a, name).await;
        let turn = Uuid::new_v4();

        let (a, b) =
            tokio::join!(claim(&replica_a, &session, turn), claim(&replica_b, &session, turn));
        let granted =
            [a.unwrap(), b.unwrap()].into_iter().filter(|c| matches!(c, Claim::Granted)).count();
        assert_eq!(granted, 1, "the loser must not dispatch its own copy");
    }

    #[tokio::test]
    async fn a_confirmed_claim_is_swept_once_it_ages_out() {
        let name = "turn_dedupe_sweep";
        let Some(pool) = pool_for(name).await else { return };
        let session = seed(&pool, name).await;
        let turn = Uuid::new_v4();

        claim(&pool, &session, turn).await.unwrap();
        confirm(&pool, &session, turn, Uuid::new_v4()).await.unwrap();
        sqlx::query(
            "UPDATE dispatched_turns SET claimed_at = now() - interval '2 days' \
             WHERE session_id = $1",
        )
        .bind(&session)
        .execute(&pool)
        .await
        .unwrap();

        sweep(&pool).await;
        assert_eq!(
            claim(&pool, &session, turn).await.unwrap(),
            Claim::Granted,
            "a swept turn is forgotten, not blocked"
        );
    }

    /// A replica that died mid-dispatch must not wedge the turn forever.
    #[tokio::test]
    async fn an_abandoned_in_flight_claim_is_reclaimable() {
        let name = "turn_dedupe_abandoned";
        let Some(pool) = pool_for(name).await else { return };
        let session = seed(&pool, name).await;
        let turn = Uuid::new_v4();

        assert_eq!(claim(&pool, &session, turn).await.unwrap(), Claim::Granted);
        assert_eq!(
            claim(&pool, &session, turn).await.unwrap(),
            Claim::InFlight,
            "a fresh unconfirmed claim blocks a concurrent attempt"
        );

        sqlx::query(
            "UPDATE dispatched_turns SET claimed_at = now() - interval '10 minutes' \
             WHERE session_id = $1",
        )
        .bind(&session)
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            claim(&pool, &session, turn).await.unwrap(),
            Claim::Granted,
            "an abandoned claim is reclaimed, not a permanent block"
        );
    }
}
