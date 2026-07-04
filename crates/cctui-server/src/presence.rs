//! Replica-aware WS presence (CCT-567).
//!
//! The daemon/dispatcher connection registries in [`crate::state::AppState`]
//! are per-pod in-memory maps, so with multiple server replicas an HTTP request
//! that needs a live WS can land on a pod that doesn't hold it. Each replica
//! records the WS connections it terminates in the `ws_presence` table; a pod
//! that misses locally consults the table and, when a live peer owns the
//! connection, answers 421 so [`crate::forward`] re-sends the request to that
//! peer.
//!
//! Registration only happens when the pod knows its own routable IP
//! (`CCTUI_POD_IP`, injected via the k8s downward API). Without it — local dev,
//! single-replica deployments — nothing is written and behavior is exactly the
//! pre-CCT-567 single-pod model; lookups still work so such a pod can forward
//! *to* registered peers.

use uuid::Uuid;

use crate::state::AppState;

/// What kind of WS a presence row describes. Stored as text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Daemon,
    Dispatcher,
}

impl Kind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daemon => "daemon",
            Self::Dispatcher => "dispatcher",
        }
    }
}

/// A row's heartbeat must be at most this old to be trusted. Heartbeats are
/// written every [`HEARTBEAT_SECS`], so 3× distinguishes a crashed pod from a
/// slow tick (mirrors the WS read-timeout discipline, CCT-140).
const LIVE_WITHIN_SECS: i32 = 45;
/// Cadence of the per-pod heartbeat task.
const HEARTBEAT_SECS: u64 = 15;

/// This pod's identity for presence rows. Built once at boot.
pub struct PodIdentity {
    /// Pod (host) name — unique per replica; scopes our own rows.
    pub pod: String,
    /// Routable IP peers can reach this pod's HTTP port on. `None` disables
    /// registration (this pod never OWNS forwardable rows).
    pub ip: Option<String>,
}

impl PodIdentity {
    /// `CCTUI_POD_IP` (downward API) + `HOSTNAME`. No IP ⇒ registration off.
    pub fn from_env() -> Self {
        let pod = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into());
        let ip = std::env::var("CCTUI_POD_IP").ok().filter(|s| !s.trim().is_empty());
        match &ip {
            Some(ip) => tracing::info!(pod, ip, "WS presence registration enabled"),
            None => {
                tracing::info!(pod, "CCTUI_POD_IP unset — WS presence registration disabled");
            }
        }
        Self { pod, ip }
    }
}

/// Record this pod as the owner of `entity_id`'s live WS. Upsert: on a
/// cross-pod reconnect the newest connection wins, exactly like the in-memory
/// registries. No-op without a pod IP. Best-effort — a presence write failure
/// must never break the WS itself.
pub async fn register(state: &AppState, kind: Kind, entity_id: Uuid) {
    let Some(ip) = state.presence.ip.as_deref() else { return };
    if let Err(err) = sqlx::query(
        "INSERT INTO ws_presence (kind, entity_id, pod, pod_ip, connected_at, heartbeat_at) \
         VALUES ($1, $2, $3, $4, now(), now()) \
         ON CONFLICT (kind, entity_id) DO UPDATE SET \
           pod = EXCLUDED.pod, pod_ip = EXCLUDED.pod_ip, \
           connected_at = now(), heartbeat_at = now()",
    )
    .bind(kind.as_str())
    .bind(entity_id)
    .bind(&state.presence.pod)
    .bind(ip)
    .execute(&state.pool)
    .await
    {
        tracing::warn!(%err, %entity_id, kind = kind.as_str(), "ws_presence register failed");
    }
}

/// Drop this pod's presence row for `entity_id`. Guarded by `pod = self`: if
/// the entity already reconnected to a peer (which upserted the row over to
/// itself), our late disconnect cleanup must not delete the new owner's row —
/// the cross-pod twin of the `remove_if(same_channel)` guard (CCT-159).
pub async fn unregister(state: &AppState, kind: Kind, entity_id: Uuid) {
    if state.presence.ip.is_none() {
        return;
    }
    if let Err(err) =
        sqlx::query("DELETE FROM ws_presence WHERE kind = $1 AND entity_id = $2 AND pod = $3")
            .bind(kind.as_str())
            .bind(entity_id)
            .bind(&state.presence.pod)
            .execute(&state.pool)
            .await
    {
        tracing::warn!(%err, %entity_id, kind = kind.as_str(), "ws_presence unregister failed");
    }
}

/// The IP of a live PEER pod owning `entity_id`'s WS, if any. `None` means
/// "no live peer owns it" — either truly offline, or a stale row (crashed pod),
/// or we own it ourselves (callers check the in-memory registry first, so a
/// self-row here still means the connection is gone locally → offline).
pub async fn peer_owner(state: &AppState, kind: Kind, entity_id: Uuid) -> Option<String> {
    sqlx::query_scalar::<_, String>(
        "SELECT pod_ip FROM ws_presence \
         WHERE kind = $1 AND entity_id = $2 AND pod <> $3 \
           AND heartbeat_at > now() - make_interval(secs => $4)",
    )
    .bind(kind.as_str())
    .bind(entity_id)
    .bind(&state.presence.pod)
    .bind(f64::from(LIVE_WITHIN_SECS))
    .fetch_optional(&state.pool)
    .await
    .map_err(|err| tracing::warn!(%err, %entity_id, "ws_presence lookup failed"))
    .ok()
    .flatten()
}

/// Boot cleanup + heartbeat loop. On start, drop any rows a previous
/// incarnation of THIS pod name left behind (a crashed process can't
/// unregister); then refresh our rows' heartbeats every [`HEARTBEAT_SECS`] and
/// opportunistically reap long-dead rows from crashed peers so the table stays
/// small. Spawned from `main` only when registration is enabled.
pub async fn heartbeat_task(state: AppState) {
    if let Err(err) = sqlx::query("DELETE FROM ws_presence WHERE pod = $1")
        .bind(&state.presence.pod)
        .execute(&state.pool)
        .await
    {
        tracing::warn!(%err, "ws_presence boot cleanup failed");
    }
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(HEARTBEAT_SECS));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        interval.tick().await;
        if let Err(err) = sqlx::query("UPDATE ws_presence SET heartbeat_at = now() WHERE pod = $1")
            .bind(&state.presence.pod)
            .execute(&state.pool)
            .await
        {
            tracing::warn!(%err, "ws_presence heartbeat failed");
        }
        // Rows a crashed pod never deleted: long past any liveness window, safe
        // for anyone to reap (idempotent across replicas).
        let _ = sqlx::query(
            "DELETE FROM ws_presence WHERE heartbeat_at < now() - interval '10 minutes'",
        )
        .execute(&state.pool)
        .await;
    }
}
