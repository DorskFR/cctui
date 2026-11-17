use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::preset::lane as preset_lane;
use super::store::{self, NewEvent};
use crate::presets::cron;
use crate::routes::inbox::DEFAULT_TOPIC;
use crate::state::AppState;

const BATCH: i64 = 50;

/// Two replicas firing the same tick produce the same id, so the insert dedups.
#[must_use]
pub fn source_id(preset_id: Uuid, fire_at: DateTime<Utc>) -> String {
    format!("{preset_id}@{}", fire_at.format("%Y-%m-%dT%H:%M:00Z"))
}

#[derive(sqlx::FromRow)]
struct Due {
    id: Uuid,
    user_id: Uuid,
    cron: String,
    next_fire_at: DateTime<Utc>,
}

/// `Some(lane)` when this call wrote the event.
async fn fire(pool: &PgPool, due: &Due, now: DateTime<Utc>) -> sqlx::Result<Option<Uuid>> {
    let lane_id = preset_lane(pool, due.user_id, due.id, DEFAULT_TOPIC).await?;
    let fire_at = due.next_fire_at;
    let payload = serde_json::json!({
        "preset_id": due.id,
        "cron": due.cron,
        "fire_at": fire_at,
    });
    let body = format!("Scheduled run ({}) for {}", due.cron, fire_at.format("%Y-%m-%dT%H:%MZ"));
    let source_id = source_id(due.id, fire_at);
    let inserted = store::insert_event(
        pool,
        &NewEvent {
            lane_id,
            user_id: due.user_id,
            source: Source::Schedule,
            source_id: Some(&source_id),
            body: &body,
            payload: Some(&payload),
            not_before: None,
            expires_at: None,
        },
    )
    .await?;
    let next = cron::next_fire(&due.cron, fire_at.max(now));
    sqlx::query("UPDATE presets SET next_fire_at = $3 WHERE id = $1 AND next_fire_at = $2")
        .bind(due.id)
        .bind(fire_at)
        .bind(next)
        .execute(pool)
        .await?;
    Ok((!inserted.deduplicated).then_some(lane_id))
}

/// A missed window fires once, then the schedule resumes from `now`. Returns
/// the lanes that got a new event.
pub async fn fire_due(pool: &PgPool, now: DateTime<Utc>) -> sqlx::Result<Vec<Uuid>> {
    let due: Vec<Due> = sqlx::query_as(
        "SELECT id, user_id, cron, next_fire_at FROM presets \
         WHERE cron IS NOT NULL AND next_fire_at <= $1 ORDER BY next_fire_at LIMIT $2",
    )
    .bind(now)
    .bind(BATCH)
    .fetch_all(pool)
    .await?;
    let mut lanes = Vec::new();
    for row in &due {
        if let Some(lane) = fire(pool, row, now).await? {
            lanes.push(lane);
        }
    }
    Ok(lanes)
}

pub async fn backfill(pool: &PgPool, now: DateTime<Utc>) -> sqlx::Result<u64> {
    let rows: Vec<(Uuid, String)> = sqlx::query_as(
        "SELECT id, cron FROM presets WHERE cron IS NOT NULL AND next_fire_at IS NULL LIMIT $1",
    )
    .bind(BATCH)
    .fetch_all(pool)
    .await?;
    let mut set = 0;
    for (id, expr) in rows {
        let Some(next) = cron::next_fire(&expr, now) else { continue };
        set += sqlx::query(
            "UPDATE presets SET next_fire_at = $2 \
             WHERE id = $1 AND next_fire_at IS NULL AND cron = $3",
        )
        .bind(id)
        .bind(next)
        .bind(&expr)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(set)
}

pub async fn sweep(state: &AppState) {
    let now = Utc::now();
    if let Err(e) = backfill(&state.pool, now).await {
        tracing::warn!("preset schedule backfill failed: {e}");
    }
    match fire_due(&state.pool, now).await {
        Ok(lanes) => {
            for lane in lanes {
                super::deliver::wake_lane(state, lane);
            }
        }
        Err(e) => tracing::warn!("preset schedule sweep failed: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use cctui_proto::api::presets::LaunchPresetRequest;
    use chrono::{Duration, TimeZone, Utc};
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{DEFAULT_TOPIC, backfill, fire_due, preset_lane, source_id};
    use crate::presets::store_tests::seed_owner;
    use crate::presets::{store, validate};

    async fn pool(name: &str) -> Option<PgPool> {
        let url = crate::routes::gateway::test_db_url(name)?;
        Some(
            sqlx::postgres::PgPoolOptions::new()
                .max_connections(4)
                .connect(&url)
                .await
                .expect("connect test db"),
        )
    }

    fn request(machine: Uuid, cron: Option<&str>) -> LaunchPresetRequest {
        LaunchPresetRequest {
            name: format!("cron-{}", Uuid::new_v4()),
            prompt: "triage".into(),
            machine_id: Some(machine),
            working_dir: Some("/w".into()),
            cron: cron.map(str::to_owned),
            ..LaunchPresetRequest::default()
        }
    }

    async fn next_fire_at(pool: &PgPool, id: Uuid) -> Option<chrono::DateTime<Utc>> {
        sqlx::query_scalar("SELECT next_fire_at FROM presets WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[test]
    fn the_source_id_is_the_preset_and_the_minute() {
        let id = Uuid::nil();
        let at = Utc.with_ymd_and_hms(2026, 11, 16, 9, 5, 42).unwrap();
        assert_eq!(source_id(id, at), format!("{id}@2026-11-16T09:05:00Z"));
    }

    #[tokio::test]
    async fn a_cron_preset_gets_its_next_fire_time_on_create_and_update() {
        let Some(pool) = pool("a_cron_preset_gets_its_next_fire_time").await else { return };
        let owner = seed_owner(&pool, "cronset").await;
        let before = Utc::now();
        let req = request(owner.machine, Some("*/5 * * * *"));
        let preset = store::create(&pool, owner.user, &validate(&req).unwrap()).await.unwrap();
        let first = preset.next_fire_at.expect("a cron preset has a next fire time");
        assert!(first > before && first <= before + Duration::minutes(5), "{first}");

        let same = store::update(&pool, None, preset.id, &validate(&req).unwrap()).await.unwrap();
        assert_eq!(same.unwrap().next_fire_at, Some(first), "an unchanged cron keeps its time");

        let off = LaunchPresetRequest { cron: None, ..req };
        let cleared = store::update(&pool, None, preset.id, &validate(&off).unwrap()).await;
        assert_eq!(cleared.unwrap().unwrap().next_fire_at, None);

        sqlx::query("UPDATE presets SET cron = '0 9 * * *' WHERE id = $1")
            .bind(preset.id)
            .execute(&pool)
            .await
            .unwrap();
        assert!(backfill(&pool, Utc::now()).await.unwrap() >= 1);
        assert!(next_fire_at(&pool, preset.id).await.is_some());
    }

    #[tokio::test]
    async fn two_sweeps_for_the_same_tick_write_one_event() {
        let Some(pool) = pool("two_sweeps_for_the_same_tick_write_one_event").await else {
            return;
        };
        let owner = seed_owner(&pool, "crontick").await;
        let req = request(owner.machine, Some("*/5 * * * *"));
        let preset = store::create(&pool, owner.user, &validate(&req).unwrap()).await.unwrap();
        let tick = Utc.with_ymd_and_hms(2020, 1, 6, 9, 5, 0).unwrap();
        sqlx::query("UPDATE presets SET next_fire_at = $2 WHERE id = $1")
            .bind(preset.id)
            .bind(tick)
            .execute(&pool)
            .await
            .unwrap();

        let now = Utc::now();
        let (a, b) = tokio::join!(fire_due(&pool, now), fire_due(&pool, now));
        let lane = preset_lane(&pool, owner.user, preset.id, DEFAULT_TOPIC).await.unwrap();
        let woken = a.unwrap().into_iter().chain(b.unwrap()).filter(|l| *l == lane).count();
        assert_eq!(woken, 1, "exactly one sweep wrote the event");

        let events: Vec<(String, Option<String>)> = sqlx::query_as(
            "SELECT e.source, e.source_id FROM inbox_events e \
             JOIN inbox_lanes l ON l.id = e.lane_id WHERE l.preset_id = $1",
        )
        .bind(preset.id)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(
            events,
            [("schedule".to_owned(), Some(format!("{}@2020-01-06T09:05:00Z", preset.id)))]
        );
        let next = next_fire_at(&pool, preset.id).await.expect("advanced");
        assert!(next > now && next <= now + Duration::minutes(5), "{next}");
        let again = fire_due(&pool, now).await.unwrap();
        assert!(!again.contains(&lane), "nothing due until the next tick");
    }
}
