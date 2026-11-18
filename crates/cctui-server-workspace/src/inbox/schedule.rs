use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::Source;
use super::preset::{REGISTER_WAIT_SECS, lane as preset_lane};
use super::store::{self, Inserted, NewEvent};
use crate::presets::tz;
use crate::routes::inbox::DEFAULT_TOPIC;
use crate::state::AppState;

const BATCH: i64 = 50;
pub const SKIPPED: &str = "skipped: the previous run is still working";

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
    timezone: String,
    overlap: String,
    next_fire_at: DateTime<Utc>,
}

/// A run is live while its event waits, its spawn has yet to register, or its
/// session is mid-turn.
const LIVE_SQL: &str = "\
    SELECT EXISTS ( \
        SELECT 1 FROM inbox_lanes l \
        LEFT JOIN machine_launches ml ON ml.command_id = l.spawn_command_id \
        WHERE l.id = $1 AND ( \
            EXISTS (SELECT 1 FROM inbox_events e \
                    WHERE e.lane_id = l.id AND e.status IN ('pending', 'claimed')) \
            OR (ml.command_id IS NOT NULL \
                AND ml.state NOT IN ('refused', 'delivery_failed') \
                AND ml.created_at > now() - $2::bigint * interval '1 second' \
                AND NOT EXISTS (SELECT 1 FROM sessions s \
                                WHERE s.id = COALESCE(ml.registered_session_id, ml.session_id))) \
            OR EXISTS ( \
                SELECT 1 FROM sessions s \
                WHERE s.id IN (l.session_id, \
                               COALESCE(ml.registered_session_id, ml.session_id), \
                               (SELECT e.delivered_session_id FROM inbox_events e \
                                WHERE e.lane_id = l.id AND e.status = 'delivered' \
                                ORDER BY e.seq DESC LIMIT 1)) \
                  AND s.status NOT IN ('ended', 'archived', 'failed', 'inactive') \
                  AND (COALESCE(s.tempo, '') IN ('blocked', 'active') \
                       OR COALESCE(s.agent_state, '') IN ('working', 'running')))))";

pub async fn run_is_live(pool: &PgPool, lane_id: Uuid) -> sqlx::Result<bool> {
    sqlx::query_scalar(LIVE_SQL).bind(lane_id).bind(REGISTER_WAIT_SECS).fetch_one(pool).await
}

async fn record_skip(
    pool: &PgPool,
    due: &Due,
    lane_id: Uuid,
    source_id: &str,
    body: &str,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO inbox_events \
           (lane_id, user_id, source, source_id, body, status, last_error) \
         VALUES ($1, $2, 'schedule', $3, $4, 'cancelled', $5) \
         ON CONFLICT (lane_id, source, source_id) DO NOTHING",
    )
    .bind(lane_id)
    .bind(due.user_id)
    .bind(source_id)
    .bind(body)
    .bind(SKIPPED)
    .execute(pool)
    .await
    .map(|_| ())
}

/// `Some(lane)` when this call wrote an event to deliver.
async fn fire(pool: &PgPool, due: &Due, now: DateTime<Utc>) -> sqlx::Result<Option<Uuid>> {
    let lane_id = preset_lane(pool, due.user_id, due.id, DEFAULT_TOPIC).await?;
    let fire_at = due.next_fire_at;
    let body = format!("Scheduled run ({}) for {}", due.cron, fire_at.format("%Y-%m-%dT%H:%MZ"));
    let source_id = source_id(due.id, fire_at);
    let written = if due.overlap == "skip" && run_is_live(pool, lane_id).await? {
        record_skip(pool, due, lane_id, &source_id, &body).await?;
        false
    } else {
        let payload = serde_json::json!({
            "preset_id": due.id,
            "cron": due.cron,
            "timezone": due.timezone,
            "fire_at": fire_at,
        });
        let event = NewEvent {
            lane_id,
            user_id: due.user_id,
            source: Source::Schedule,
            source_id: Some(&source_id),
            body: &body,
            payload: Some(&payload),
            not_before: None,
            expires_at: None,
        };
        !store::insert_event(pool, &event).await?.deduplicated
    };
    let next = tz::next_fire_in(pool, &due.cron, &due.timezone, fire_at.max(now)).await?;
    sqlx::query(
        "UPDATE presets SET next_fire_at = $3, last_fire_at = $2 \
         WHERE id = $1 AND next_fire_at = $2 AND enabled",
    )
    .bind(due.id)
    .bind(fire_at)
    .bind(next)
    .execute(pool)
    .await?;
    Ok(written.then_some(lane_id))
}

/// A missed window fires once, then the schedule resumes from `now`. Returns
/// the lanes that got a new event.
pub async fn fire_due(pool: &PgPool, now: DateTime<Utc>) -> sqlx::Result<Vec<Uuid>> {
    fire_due_of(pool, now, None).await
}

/// `only` narrows the sweep to one preset.
async fn fire_due_of(
    pool: &PgPool,
    now: DateTime<Utc>,
    only: Option<Uuid>,
) -> sqlx::Result<Vec<Uuid>> {
    let due: Vec<Due> = sqlx::query_as(
        "SELECT id, user_id, cron, timezone, overlap, next_fire_at FROM presets \
         WHERE cron IS NOT NULL AND enabled AND next_fire_at <= $1 \
           AND ($3::uuid IS NULL OR id = $3) \
         ORDER BY next_fire_at LIMIT $2",
    )
    .bind(now)
    .bind(BATCH)
    .bind(only)
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
    backfill_of(pool, now, None).await
}

async fn backfill_of(pool: &PgPool, now: DateTime<Utc>, only: Option<Uuid>) -> sqlx::Result<u64> {
    let rows: Vec<(Uuid, String, String)> = sqlx::query_as(
        "SELECT id, cron, timezone FROM presets \
         WHERE cron IS NOT NULL AND enabled AND next_fire_at IS NULL \
           AND ($2::uuid IS NULL OR id = $2) LIMIT $1",
    )
    .bind(BATCH)
    .bind(only)
    .fetch_all(pool)
    .await?;
    let mut set = 0;
    for (id, expr, timezone) in rows {
        let Some(next) = tz::next_fire_in(pool, &expr, &timezone, now).await? else { continue };
        set += sqlx::query(
            "UPDATE presets SET next_fire_at = $2 \
             WHERE id = $1 AND next_fire_at IS NULL AND enabled AND cron = $3 AND timezone = $4",
        )
        .bind(id)
        .bind(next)
        .bind(&expr)
        .bind(&timezone)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(set)
}

/// A manual run ignores the pause and the overlap policy. `None` when the
/// caller sees no such preset.
pub async fn run_now(
    pool: &PgPool,
    owner: Option<Uuid>,
    preset_id: Uuid,
) -> sqlx::Result<Option<(Uuid, Inserted)>> {
    let found: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT user_id, name FROM presets WHERE id = $1 AND ($2::uuid IS NULL OR user_id = $2)",
    )
    .bind(preset_id)
    .bind(owner)
    .fetch_optional(pool)
    .await?;
    let Some((user_id, name)) = found else { return Ok(None) };
    let lane_id = preset_lane(pool, user_id, preset_id, DEFAULT_TOPIC).await?;
    let source_id = format!("{preset_id}@manual-{}", Uuid::new_v4());
    let payload = serde_json::json!({ "preset_id": preset_id, "manual": true });
    let body = format!("Manual run of {name}");
    let event = NewEvent {
        lane_id,
        user_id,
        source: Source::Schedule,
        source_id: Some(&source_id),
        body: &body,
        payload: Some(&payload),
        not_before: None,
        expires_at: None,
    };
    Ok(Some((lane_id, store::insert_event(pool, &event).await?)))
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
    use cctui_proto::api::presets::{LaunchPresetRequest, Overlap};
    use chrono::{Duration, TimeZone, Utc};
    use sqlx::PgPool;
    use uuid::Uuid;

    use super::{
        DEFAULT_TOPIC, SKIPPED, backfill_of, fire_due_of, preset_lane, run_now, source_id,
    };
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
        assert_eq!(backfill_of(&pool, Utc::now(), Some(preset.id)).await.unwrap(), 1);
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

        let (now, only) = (Utc::now(), Some(preset.id));
        let (a, b) = tokio::join!(fire_due_of(&pool, now, only), fire_due_of(&pool, now, only));
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
        let again = fire_due_of(&pool, now, only).await.unwrap();
        assert!(!again.contains(&lane), "nothing due until the next tick");
    }
    async fn set_tick(pool: &PgPool, id: Uuid, tick: chrono::DateTime<Utc>) {
        sqlx::query("UPDATE presets SET next_fire_at = $2 WHERE id = $1")
            .bind(id)
            .bind(tick)
            .execute(pool)
            .await
            .unwrap();
    }

    async fn events(pool: &PgPool, preset: Uuid) -> Vec<(String, Option<String>)> {
        sqlx::query_as(
            "SELECT e.status, e.last_error FROM inbox_events e \
             JOIN inbox_lanes l ON l.id = e.lane_id WHERE l.preset_id = $1 ORDER BY e.seq",
        )
        .bind(preset)
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn a_tokyo_weekday_schedule_fires_once_per_tick_across_two_replicas() {
        let Some(pool) = pool("a_tokyo_weekday_schedule_fires_once").await else { return };
        let owner = seed_owner(&pool, "crontokyo").await;
        let req = LaunchPresetRequest {
            timezone: Some("Asia/Tokyo".into()),
            ..request(owner.machine, Some("0 7 * * 1-5"))
        };
        let preset = store::create(&pool, owner.user, &validate(&req).unwrap()).await.unwrap();
        let first = preset.next_fire_at.expect("scheduled");
        assert_eq!(first.format("%H:%M").to_string(), "22:00", "07:00 JST is 22:00 UTC");
        assert_eq!(preset.timezone, "Asia/Tokyo");

        let monday_seven_jst = Utc.with_ymd_and_hms(2026, 11, 15, 22, 0, 0).unwrap();
        set_tick(&pool, preset.id, monday_seven_jst).await;
        let (now, only) = (Utc::now(), Some(preset.id));
        let (a, b) = tokio::join!(fire_due_of(&pool, now, only), fire_due_of(&pool, now, only));
        assert_eq!(a.unwrap().len() + b.unwrap().len(), 1, "one replica wrote the event");
        assert_eq!(events(&pool, preset.id).await, [("pending".to_owned(), None)]);

        let after = store::get(&pool, None, preset.id).await.unwrap().unwrap();
        assert_eq!(after.last_fire_at, Some(monday_seven_jst));
        let next = after.next_fire_at.expect("advanced");
        assert!(next > now && next <= now + Duration::days(3), "{next}");
        assert_eq!(next.format("%H:%M").to_string(), "22:00");
        fire_due_of(&pool, Utc::now(), only).await.unwrap();
        assert_eq!(events(&pool, preset.id).await.len(), 1, "a restart does not fire again");
    }

    #[tokio::test]
    async fn a_paused_preset_does_not_fire_and_resumes_from_now() {
        let Some(pool) = pool("a_paused_preset_does_not_fire").await else { return };
        let owner = seed_owner(&pool, "cronpause").await;
        let req = request(owner.machine, Some("*/5 * * * *"));
        let preset = store::create(&pool, owner.user, &validate(&req).unwrap()).await.unwrap();

        let paused = LaunchPresetRequest { enabled: Some(false), ..req.clone() };
        let off = store::update(&pool, None, preset.id, &validate(&paused).unwrap()).await;
        let off = off.unwrap().unwrap();
        assert_eq!((off.enabled, off.next_fire_at), (false, None));

        set_tick(&pool, preset.id, Utc.with_ymd_and_hms(2020, 1, 6, 9, 5, 0).unwrap()).await;
        let now = Utc::now();
        fire_due_of(&pool, now, Some(preset.id)).await.unwrap();
        assert!(events(&pool, preset.id).await.is_empty(), "a paused cron does not fire");
        sqlx::query("UPDATE presets SET next_fire_at = NULL WHERE id = $1")
            .bind(preset.id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(backfill_of(&pool, now, Some(preset.id)).await.unwrap(), 0);
        assert_eq!(next_fire_at(&pool, preset.id).await, None, "nor is it backfilled");

        let on = store::update(&pool, None, preset.id, &validate(&req).unwrap()).await;
        let next = on.unwrap().unwrap().next_fire_at.expect("resumed");
        assert!(next > now && next <= Utc::now() + Duration::minutes(5), "no catch-up: {next}");
    }

    async fn tick(pool: &PgPool, preset: Uuid, minute: u32) -> Vec<String> {
        set_tick(pool, preset, Utc.with_ymd_and_hms(2020, 1, 6, 9, minute, 0).unwrap()).await;
        fire_due_of(pool, Utc::now(), Some(preset)).await.unwrap();
        events(pool, preset).await.into_iter().map(|(status, _)| status).collect()
    }

    #[tokio::test]
    async fn skip_overlap_drops_a_fire_while_the_previous_run_is_live() {
        let Some(pool) = pool("skip_overlap_drops_a_fire").await else { return };
        let owner = seed_owner(&pool, "cronskip").await;
        let req = LaunchPresetRequest {
            overlap: Some(Overlap::Skip),
            ..request(owner.machine, Some("*/5 * * * *"))
        };
        let preset = store::create(&pool, owner.user, &validate(&req).unwrap()).await.unwrap();

        assert_eq!(tick(&pool, preset.id, 5).await, ["pending"], "nothing is running yet");
        assert_eq!(tick(&pool, preset.id, 10).await, ["pending", "cancelled"], "it still waits");
        let skipped = events(&pool, preset.id).await.pop().unwrap();
        assert_eq!(skipped.1.as_deref(), Some(SKIPPED));

        let session = format!("run-{}", preset.id);
        sqlx::query(
            "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status, \
             adapter_id, agent_state) VALUES ($1, $2, $2, $3, '/w', 'active', 'claude-code', \
             'working')",
        )
        .bind(&session)
        .bind(owner.machine)
        .bind(owner.user)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE inbox_events e SET status = 'delivered', delivered_session_id = $2 \
             FROM inbox_lanes l WHERE l.id = e.lane_id AND l.preset_id = $1 \
               AND e.status = 'pending'",
        )
        .bind(preset.id)
        .bind(&session)
        .execute(&pool)
        .await
        .unwrap();
        let mid_turn = tick(&pool, preset.id, 15).await;
        assert_eq!(mid_turn, ["delivered", "cancelled", "cancelled"], "its session is working");

        sqlx::query("UPDATE sessions SET agent_state = 'idle' WHERE id = $1")
            .bind(&session)
            .execute(&pool)
            .await
            .unwrap();
        let finished = tick(&pool, preset.id, 20).await;
        assert_eq!(finished, ["delivered", "cancelled", "cancelled", "pending"]);

        let allow = request(owner.machine, Some("*/5 * * * *"));
        let allow = store::create(&pool, owner.user, &validate(&allow).unwrap()).await.unwrap();
        tick(&pool, allow.id, 5).await;
        assert_eq!(tick(&pool, allow.id, 10).await, ["pending", "pending"], "allow queues");
    }

    #[tokio::test]
    async fn run_now_queues_a_run_for_a_paused_preset_of_the_caller_only() {
        let Some(pool) = pool("run_now_queues_a_run").await else { return };
        let (owner, other) =
            (seed_owner(&pool, "cronrun").await, seed_owner(&pool, "cronrun2").await);
        let req = LaunchPresetRequest {
            enabled: Some(false),
            overlap: Some(Overlap::Skip),
            ..request(owner.machine, Some("0 9 * * *"))
        };
        let preset = store::create(&pool, owner.user, &validate(&req).unwrap()).await.unwrap();
        assert!(run_now(&pool, Some(other.user), preset.id).await.unwrap().is_none());
        let (lane, first) = run_now(&pool, Some(owner.user), preset.id).await.unwrap().unwrap();
        let (_, second) = run_now(&pool, None, preset.id).await.unwrap().unwrap();
        assert!(!first.deduplicated && !second.deduplicated && first.id != second.id);
        assert_eq!(lane, preset_lane(&pool, owner.user, preset.id, DEFAULT_TOPIC).await.unwrap());
        let sources: Vec<String> = sqlx::query_scalar(
            "SELECT source FROM inbox_events WHERE lane_id = $1 AND status = 'pending'",
        )
        .bind(lane)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(sources, ["schedule", "schedule"]);
    }

    #[tokio::test]
    async fn an_unknown_timezone_is_refused() {
        let Some(pool) = pool("an_unknown_timezone_is_refused").await else { return };
        let owner = seed_owner(&pool, "crontz").await;
        let req = LaunchPresetRequest {
            timezone: Some("Mars/Olympus".into()),
            ..request(owner.machine, Some("0 9 * * *"))
        };
        let refused = store::create(&pool, owner.user, &validate(&req).unwrap()).await;
        let Err(store::WriteError::Invalid(why)) = refused else { panic!("{refused:?}") };
        assert!(why.contains("Mars/Olympus"), "{why}");
    }
}
