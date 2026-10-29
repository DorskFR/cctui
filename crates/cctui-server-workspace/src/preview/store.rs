//! The cluster-wide half of the preview registry: which previews exist, who
//! owns them, and which ticket nonces have been burnt. Every replica reads and
//! writes this; the tunnel's stream state stays in the owning pod's memory.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::{MAX_PER_SESSION, Preview};

/// How long a preview survives its daemon disconnecting. Long enough for a
/// rolling restart to bring the daemon back on another pod.
pub const DETACH_GRACE_SECS: i64 = 120;

fn row_to_preview(row: &sqlx::postgres::PgRow) -> Preview {
    use sqlx::Row;
    Preview {
        id: row.get("id"),
        session_id: row.get("session_id"),
        user_id: row.get("user_id"),
        machine_id: row.get("machine_id"),
        port: u16::try_from(row.get::<i32, _>("port")).unwrap_or(0),
        opened_at: row.get("created_at"),
    }
}

pub async fn lookup(pool: &PgPool, id: &str) -> Option<Preview> {
    sqlx::query(
        "SELECT id, session_id, user_id, machine_id, port, created_at FROM previews WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await
    .map_err(|e| tracing::warn!(%e, %id, "preview lookup failed"))
    .ok()
    .flatten()
    .as_ref()
    .map(row_to_preview)
}

pub async fn list_session(pool: &PgPool, session_id: &str) -> Vec<Preview> {
    sqlx::query(
        "SELECT id, session_id, user_id, machine_id, port, created_at FROM previews \
         WHERE session_id = $1 ORDER BY created_at",
    )
    .bind(session_id)
    .fetch_all(pool)
    .await
    .map_err(|e| tracing::warn!(%e, %session_id, "preview list failed"))
    .unwrap_or_default()
    .iter()
    .map(row_to_preview)
    .collect()
}

pub async fn count_session(pool: &PgPool, session_id: &str) -> usize {
    sqlx::query_scalar::<_, i64>("SELECT count(*) FROM previews WHERE session_id = $1")
        .bind(session_id)
        .fetch_one(pool)
        .await
        .map_or(0, |n| usize::try_from(n).unwrap_or(0))
}

pub async fn by_port(pool: &PgPool, session_id: &str, port: u16) -> Option<Preview> {
    sqlx::query(
        "SELECT id, session_id, user_id, machine_id, port, created_at FROM previews \
         WHERE session_id = $1 AND port = $2",
    )
    .bind(session_id)
    .bind(i32::from(port))
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .as_ref()
    .map(row_to_preview)
}

/// A daemon re-announcing `requested_id` after a reconnect: adopt the existing
/// row only when it is the very same preview (machine, session, user, port),
/// clearing the detach marker. Anything else is refused so a daemon cannot
/// claim another session's preview id.
pub async fn rebind(
    pool: &PgPool,
    requested_id: &str,
    session_id: &str,
    user_id: Uuid,
    machine_id: Uuid,
    port: u16,
) -> Option<Preview> {
    sqlx::query(
        "UPDATE previews SET detached_at = NULL \
         WHERE id = $1 AND session_id = $2 AND user_id = $3 AND machine_id = $4 AND port = $5 \
         RETURNING id, session_id, user_id, machine_id, port, created_at",
    )
    .bind(requested_id)
    .bind(session_id)
    .bind(user_id)
    .bind(machine_id)
    .bind(i32::from(port))
    .fetch_optional(pool)
    .await
    .map_err(|e| tracing::warn!(%e, %requested_id, "preview rebind failed"))
    .ok()
    .flatten()
    .as_ref()
    .map(row_to_preview)
}

pub async fn insert(pool: &PgPool, preview: &Preview) -> Result<Preview, sqlx::Error> {
    sqlx::query(
        "INSERT INTO previews (id, session_id, user_id, machine_id, port) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (session_id, port) DO UPDATE \
             SET machine_id = EXCLUDED.machine_id, detached_at = NULL \
         RETURNING id, session_id, user_id, machine_id, port, created_at",
    )
    .bind(&preview.id)
    .bind(&preview.session_id)
    .bind(preview.user_id)
    .bind(preview.machine_id)
    .bind(i32::from(preview.port))
    .fetch_one(pool)
    .await
    .map(|row| row_to_preview(&row))
}

/// `Err(())` when the session is already at [`MAX_PER_SESSION`].
pub async fn check_limit(pool: &PgPool, session_id: &str) -> Result<(), ()> {
    if count_session(pool, session_id).await >= MAX_PER_SESSION { Err(()) } else { Ok(()) }
}

pub async fn delete(pool: &PgPool, id: &str) {
    let _ = sqlx::query("DELETE FROM previews WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| tracing::warn!(%e, %id, "preview delete failed"));
}

pub async fn delete_session(pool: &PgPool, session_id: &str) {
    let _ = sqlx::query("DELETE FROM previews WHERE session_id = $1")
        .bind(session_id)
        .execute(pool)
        .await
        .map_err(|e| tracing::warn!(%e, %session_id, "preview session delete failed"));
}

/// Mark a disconnected daemon's previews, starting the grace period.
pub async fn detach_machine(pool: &PgPool, machine_id: Uuid) {
    let _ = sqlx::query(
        "UPDATE previews SET detached_at = now() WHERE machine_id = $1 AND detached_at IS NULL",
    )
    .bind(machine_id)
    .execute(pool)
    .await
    .map_err(|e| tracing::warn!(%e, %machine_id, "preview detach failed"));
}

pub async fn detach_session(pool: &PgPool, session_id: &str) {
    let _ = sqlx::query(
        "UPDATE previews SET detached_at = now() WHERE session_id = $1 AND detached_at IS NULL",
    )
    .bind(session_id)
    .execute(pool)
    .await
    .map_err(|e| tracing::warn!(%e, %session_id, "preview session detach failed"));
}

/// Drop previews whose daemon never came back, and expired ticket nonces.
///
/// `presence_tracked` must be false on a deployment that writes no
/// `ws_presence` rows (no `CCTUI_POD_IP`, i.e. single replica / local dev),
/// where their absence says nothing about a daemon being gone. With it true, a
/// preview left behind by a pod that died without closing its WS — so its
/// `detached_at` was never set — is reaped once no live presence row backs it.
pub async fn sweep(pool: &PgPool, presence_tracked: bool) -> u64 {
    let grace = f64::from(u32::try_from(DETACH_GRACE_SECS).unwrap_or(120));
    let dropped = sqlx::query(
        "DELETE FROM previews \
         WHERE (detached_at IS NOT NULL AND detached_at < now() - make_interval(secs => $1)) \
            OR ($2 AND created_at < now() - make_interval(secs => $1) \
                AND NOT EXISTS ( \
                    SELECT 1 FROM ws_presence p \
                    WHERE p.heartbeat_at > now() - make_interval(secs => $3) \
                      AND ((p.kind = 'daemon' AND p.entity_id = previews.machine_id) \
                        OR (p.kind = 'session' AND p.entity_id::text = previews.session_id)) \
                ))",
    )
    .bind(grace)
    .bind(presence_tracked)
    .bind(f64::from(crate::presence::LIVE_WITHIN_SECS))
    .execute(pool)
    .await
    .map_err(|e| tracing::warn!(%e, "preview sweep failed"))
    .map_or(0, |r| r.rows_affected());
    let _ = sqlx::query("DELETE FROM preview_tickets_used WHERE expires_at < now()")
        .execute(pool)
        .await
        .map_err(|e| tracing::warn!(%e, "preview nonce prune failed"));
    dropped
}

/// Burn a ticket nonce cluster-wide. `false` means it was already used.
pub async fn burn_nonce(pool: &PgPool, nonce: &str, expires_at: DateTime<Utc>) -> bool {
    sqlx::query(
        "INSERT INTO preview_tickets_used (nonce, expires_at) VALUES ($1, $2) \
         ON CONFLICT (nonce) DO NOTHING",
    )
    .bind(nonce)
    .bind(expires_at)
    .execute(pool)
    .await
    .is_ok_and(|r| r.rows_affected() == 1)
}
