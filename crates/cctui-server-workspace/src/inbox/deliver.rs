use std::fmt::Write as _;

use uuid::Uuid;

use super::claim::{self, BatchEvent, ClaimedLane, LaneFilter};
use crate::outbox::{CLAIM_BATCH, Retry};
use crate::state::AppState;
use crate::store::sessions::SessionRowStatus;
use crate::turn_dedupe::Claim;

pub const MARKER: &str = "[cctui inbox:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    Reply(String),
    QueuedSpawn(String),
    Preset(Uuid),
    Undeliverable(&'static str),
}

#[must_use]
pub fn route(lane: &ClaimedLane) -> Route {
    if let Some(preset_id) = lane.preset_id {
        return Route::Preset(preset_id);
    }
    let status = lane.session_status.as_deref();
    if let Some(reason) = crate::scheduled_messages::undeliverable_reason(status) {
        return Route::Undeliverable(reason);
    }
    let Some(session_id) = lane.session_id.clone() else {
        return Route::Undeliverable("session no longer exists");
    };
    if status == Some(SessionRowStatus::Queued.as_str()) {
        Route::QueuedSpawn(session_id)
    } else {
        Route::Reply(session_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Delivered {
        session_id: String,
        delivered_as: &'static str,
    },
    /// Not attempted: the events go back to pending with no attempt counted.
    Skip,
    Retry(String),
    /// The attempt may have landed: retried under the same `batch_id`.
    RetryKept(String),
    Dead(String),
}

const UNSEALABLE: &str = "a sealed inbox event could not be decrypted with the vault key";

/// `key` is read only when an event is sealed.
pub fn batch_text(
    events: &[BatchEvent],
    key: impl FnOnce() -> Vec<u8>,
) -> Result<String, &'static str> {
    let mut key = Some(key);
    let mut vault: Option<Vec<u8>> = None;
    let noun = if events.len() == 1 { "event" } else { "events" };
    let mut text = format!("{MARKER} {} {noun}]", events.len());
    for (n, event) in events.iter().enumerate() {
        let body = event.sealed_body.as_deref().map_or_else(
            || Ok(event.body.clone()),
            |sealed| {
                let vault =
                    vault.get_or_insert_with(|| key.take().map(|k| k()).unwrap_or_default());
                if vault.is_empty() {
                    return Err(UNSEALABLE);
                }
                crate::crypto::decrypt(sealed, vault).ok_or(UNSEALABLE)
            },
        )?;
        let at = event.created_at.format("%Y-%m-%dT%H:%MZ");
        let _ = write!(text, "\n{}. ({}, {at}) {body}", n + 1, event.source);
    }
    Ok(text)
}

async fn usage_too_high(state: &AppState, session_id: &str) -> bool {
    for account_id in crate::routes::gateway::resolve_session_accounts(state, session_id).await {
        let windows = crate::routes::gateway::usage_for_soft_limit(state, account_id)
            .await
            .as_ref()
            .map(crate::soft_limit::normalize_usage_windows)
            .unwrap_or_default();
        if crate::keepalive::usage_too_high(&windows) {
            return true;
        }
    }
    false
}

pub(super) async fn reply(state: &AppState, session_id: &str, events: &[BatchEvent]) -> Outcome {
    let Some(batch_id) = events.first().map(|e| e.batch_id) else { return Outcome::Skip };
    if usage_too_high(state, session_id).await {
        return Outcome::Skip;
    }
    let text = match batch_text(events, crate::crypto::vault_key) {
        Ok(text) => text,
        Err(reason) => return Outcome::Dead(reason.to_owned()),
    };
    let delivered =
        || Outcome::Delivered { session_id: session_id.to_owned(), delivered_as: "reply" };
    let claimed = match crate::turn_dedupe::claim(&state.pool, session_id, batch_id).await {
        Ok(Claim::Granted) => true,
        Ok(Claim::AlreadyDispatched { .. }) => return delivered(),
        Ok(Claim::InFlight) => return Outcome::Skip,
        Err(e) => {
            tracing::warn!(%e, %session_id, %batch_id, "inbox: claiming the turn failed");
            false
        }
    };
    let (command_id, dispatch) =
        crate::ws::dispatch_reply(state, session_id, text, None, Some(batch_id)).await;
    match dispatch {
        Ok(()) => {
            if claimed {
                let _ = crate::turn_dedupe::confirm(&state.pool, session_id, batch_id, command_id)
                    .await;
            }
            delivered()
        }
        Err(err) => {
            if claimed {
                let _ = crate::turn_dedupe::release(&state.pool, session_id, batch_id).await;
            }
            Outcome::Retry(err.to_string())
        }
    }
}

fn retried(retry: &Retry, batch_id: Uuid, err: String) -> Option<String> {
    match retry {
        Retry::Dead { .. } => Some(err),
        Retry::After { secs, .. } => {
            tracing::warn!(%batch_id, retry_in_secs = *secs, "inbox delivery failed: {err}");
            None
        }
    }
}

async fn settle(
    state: &AppState,
    lane: &ClaimedLane,
    route: &Route,
    batch: (Uuid, i32),
    outcome: Outcome,
) -> sqlx::Result<()> {
    let (batch_id, attempts) = batch;
    let pool = &state.pool;
    let dead = match outcome {
        Outcome::Delivered { session_id, delivered_as } => {
            claim::mark_delivered(pool, batch_id, &session_id, delivered_as).await?;
            None
        }
        Outcome::Skip => {
            claim::defer(pool, batch_id).await?;
            None
        }
        Outcome::Dead(reason) => {
            claim::mark_dead(pool, batch_id, attempts + 1, &reason).await?;
            Some(reason)
        }
        Outcome::Retry(err) => {
            let retry = claim::record_failure(pool, batch_id, attempts, &err).await?;
            retried(&retry, batch_id, err)
        }
        Outcome::RetryKept(err) => {
            let retry = claim::record_ambiguous_failure(pool, batch_id, attempts, &err).await?;
            retried(&retry, batch_id, err)
        }
    };
    if let Some(reason) = dead {
        tracing::error!(%batch_id, "inbox batch dead-lettered: {reason}");
        if let Route::QueuedSpawn(session_id) = route {
            super::queued::fail(pool, session_id, &reason).await?;
        }
        super::guards::alert(state, super::guards::dead_lettered(lane, batch_id, &reason));
    }
    Ok(())
}

async fn deliver_claimed(state: &AppState, lane: &ClaimedLane) -> sqlx::Result<()> {
    if let Some(notice) = super::guards::trip_ceiling(&state.pool, lane.id).await? {
        super::guards::alert(state, notice);
        return Ok(());
    }
    let route = route(lane);
    let events = claim::claim_batch(&state.pool, lane.id).await?;
    let Some(batch_id) = events.first().map(|e| e.batch_id) else { return Ok(()) };
    let attempts = events.iter().map(|e| e.attempts).max().unwrap_or_default();
    let outcome = match &route {
        Route::Reply(session_id) => reply(state, session_id, &events).await,
        Route::QueuedSpawn(session_id) => super::queued::launch(state, session_id, &events).await,
        Route::Preset(preset_id) => {
            super::preset::deliver(state, lane, *preset_id, &events).await?
        }
        Route::Undeliverable(reason) => Outcome::Dead((*reason).to_owned()),
    };
    settle(state, lane, &route, (batch_id, attempts), outcome).await
}

pub async fn deliver_lane(state: &AppState, lane: &ClaimedLane) {
    if let Err(e) = deliver_claimed(state, lane).await {
        tracing::warn!(lane = %lane.key, "inbox delivery failed: {e}");
    }
    if let Err(e) = claim::release_lane(&state.pool, lane.id).await {
        tracing::warn!(lane = %lane.key, "inbox lane release failed: {e}");
    }
}

pub async fn deliver_due(state: &AppState, limit: i64, filter: LaneFilter) {
    let lanes = match claim::claim_lanes(&state.pool, limit, filter).await {
        Ok(lanes) => lanes,
        Err(e) => {
            tracing::warn!("inbox lane claim failed: {e}");
            return;
        }
    };
    futures_util::future::join_all(lanes.iter().map(|lane| deliver_lane(state, lane))).await;
}

pub fn wake_lane(state: &AppState, lane_id: Uuid) {
    let state = state.clone();
    tokio::spawn(async move {
        deliver_due(&state, 1, LaneFilter { lane: Some(lane_id), machine: None }).await;
    });
}

pub fn wake_machine(state: &AppState, machine_id: Uuid) {
    let state = state.clone();
    tokio::spawn(async move {
        deliver_due(&state, CLAIM_BATCH, LaneFilter { lane: None, machine: Some(machine_id) })
            .await;
    });
}

fn log_step(step: &str, done: sqlx::Result<u64>) {
    match done {
        Ok(0) => {}
        Ok(n) => tracing::debug!(rows = n, "inbox: {step}"),
        Err(e) => tracing::warn!("inbox {step} failed: {e}"),
    }
}

pub async fn sweep(state: &AppState) {
    super::store::sweep(&state.pool).await;
    log_step("reclaimed stranded events", claim::reclaim_stranded(&state.pool).await);
    log_step(
        "expired queued spawns",
        super::queued::expire(&state.pool)
            .await
            .map(|ids| u64::try_from(ids.len()).unwrap_or(u64::MAX)),
    );
    log_step("cancelled expired events", claim::cancel_expired(&state.pool).await);
    super::guards::sweep(state).await;
    deliver_due(state, CLAIM_BATCH, LaneFilter::default()).await;
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn lane(session_status: Option<&str>, preset: bool) -> ClaimedLane {
        ClaimedLane {
            id: Uuid::nil(),
            user_id: Uuid::nil(),
            key: "session:s-1".into(),
            preset_id: preset.then(Uuid::nil),
            session_id: session_status.map(|_| "s-1".to_owned()),
            session_status: session_status.map(ToOwned::to_owned),
            spawn_command_id: None,
        }
    }

    fn event(seq: i64, source: &str, body: &str) -> BatchEvent {
        BatchEvent {
            seq,
            user_id: Uuid::nil(),
            source: source.into(),
            body: body.into(),
            sealed_body: None,
            parent_session_id: None,
            attempts: 0,
            batch_id: Uuid::nil(),
            created_at: chrono::Utc.with_ymd_and_hms(2026, 11, 16, 9, 2, 30).unwrap(),
        }
    }

    #[test]
    fn session_lanes_reply_launch_or_dead_letter_and_preset_lanes_route_by_preset() {
        assert_eq!(route(&lane(Some("active"), false)), Route::Reply("s-1".into()));
        assert_eq!(route(&lane(Some("inactive"), false)), Route::Reply("s-1".into()));
        assert_eq!(route(&lane(Some("queued"), false)), Route::QueuedSpawn("s-1".into()));
        assert_eq!(route(&lane(Some("active"), true)), Route::Preset(Uuid::nil()));
        assert_eq!(route(&lane(None, true)), Route::Preset(Uuid::nil()));
        assert_eq!(
            route(&lane(Some("archived"), false)),
            Route::Undeliverable("session archived before delivery")
        );
        assert_eq!(route(&lane(None, false)), Route::Undeliverable("session no longer exists"));
    }

    #[test]
    fn a_batch_is_one_marked_turn_in_seq_order() {
        let events = [event(1, "n8n", "ticket moved"), event(2, "timer", "check CI")];
        let text = batch_text(&events, Vec::new).unwrap();
        assert_eq!(
            text,
            "[cctui inbox: 2 events]\n1. (n8n, 2026-11-16T09:02Z) ticket moved\n\
             2. (timer, 2026-11-16T09:02Z) check CI"
        );
        assert!(batch_text(&events[..1], Vec::new).unwrap().starts_with("[cctui inbox: 1 event]"));
    }

    #[test]
    fn a_sealed_event_without_a_vault_key_is_unsealable() {
        let mut sealed = event(1, "human", "<masked>");
        sealed.sealed_body = Some("v1:00".into());
        assert_eq!(batch_text(&[sealed], Vec::new), Err(UNSEALABLE));
    }
}
