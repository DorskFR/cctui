use cctui_proto::adapter::PermissionMode;
use cctui_proto::api::presets::LaunchPresetRequest;
use sqlx::PgPool;
use uuid::Uuid;

use super::store::{WriteError, create, delete, get, list, update};
use super::{ValidPreset, validate};

async fn pool(name: &str) -> Option<PgPool> {
    let url = crate::routes::gateway::test_db_url(name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

pub struct Owner {
    pub user: Uuid,
    pub machine: Uuid,
    pub dispatcher: Uuid,
}

pub async fn seed_owner(pool: &PgPool, name: &str) -> Owner {
    let user_id = Uuid::new_v4();
    let machine_id = Uuid::new_v4();
    let dispatcher_id = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(user_id)
        .bind(format!("{name}-{user_id}"))
        .bind(format!("h-{user_id}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine_id)
        .bind(user_id)
        .bind(format!("mk-{machine_id}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO dispatchers (id, user_id, name, kind) VALUES ($1, $2, $3, 'kube')")
        .bind(dispatcher_id)
        .bind(user_id)
        .bind(format!("d-{dispatcher_id}"))
        .execute(pool)
        .await
        .unwrap();
    Owner { user: user_id, machine: machine_id, dispatcher: dispatcher_id }
}

fn machine_request(owner: &Owner, name: &str) -> LaunchPresetRequest {
    LaunchPresetRequest {
        name: name.into(),
        prompt: "triage".into(),
        machine_id: Some(owner.machine),
        working_dir: Some("/w".into()),
        ..LaunchPresetRequest::default()
    }
}

fn machine(owner: &Owner, name: &str) -> ValidPreset {
    validate(&machine_request(owner, name)).unwrap()
}

async fn lane_count(pool: &PgPool, preset_id: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM inbox_lanes WHERE preset_id = $1")
        .bind(preset_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn preset_crud_is_owner_scoped() {
    let Some(pool) = pool("preset_crud_is_owner_scoped").await else { return };
    let (a, b) = (seed_owner(&pool, "pa").await, seed_owner(&pool, "pb").await);
    let preset = create(&pool, a.user, &machine(&a, "nightly")).await.unwrap();
    assert_eq!(preset.user_id, a.user);
    assert_eq!(preset.working_dir.as_deref(), Some("/w"));

    assert!(list(&pool, Some(b.user)).await.unwrap().iter().all(|p| p.id != preset.id));
    assert_eq!(get(&pool, Some(b.user), preset.id).await.unwrap(), None);
    let edit = machine(&a, "renamed");
    assert!(matches!(update(&pool, Some(b.user), preset.id, &edit).await, Ok(None)));
    assert!(!delete(&pool, Some(b.user), preset.id).await.unwrap());

    let mine = list(&pool, Some(a.user)).await.unwrap();
    assert_eq!(mine.iter().map(|p| p.id).collect::<Vec<_>>(), [preset.id]);
    assert!(list(&pool, None).await.unwrap().iter().any(|p| p.id == preset.id), "admin view");
    let renamed = update(&pool, Some(a.user), preset.id, &edit).await.unwrap().unwrap();
    assert_eq!(renamed.name, "renamed");
    assert!(renamed.updated_at >= preset.updated_at);
    assert!(delete(&pool, Some(a.user), preset.id).await.unwrap());
    assert_eq!(get(&pool, None, preset.id).await.unwrap(), None);
}

#[tokio::test]
async fn preset_names_are_unique_per_owner() {
    let Some(pool) = pool("preset_names_are_unique_per_owner").await else { return };
    let (a, b) = (seed_owner(&pool, "na").await, seed_owner(&pool, "nb").await);
    create(&pool, a.user, &machine(&a, "same")).await.unwrap();
    let again = create(&pool, a.user, &machine(&a, "same")).await;
    assert!(matches!(again, Err(WriteError::NameTaken)), "{again:?}");
    create(&pool, b.user, &machine(&b, "same")).await.unwrap();
}

#[tokio::test]
async fn preset_fresh_secs_is_clamped_on_write() {
    let Some(pool) = pool("preset_fresh_secs_is_clamped_on_write").await else { return };
    let a = seed_owner(&pool, "fa").await;
    for (raw, want) in [(Some(5), 60), (None, 900), (Some(10_000), 2700)] {
        let name = format!("f{want}");
        let req = LaunchPresetRequest { fresh_secs: raw, ..machine_request(&a, &name) };
        let preset = create(&pool, a.user, &validate(&req).unwrap()).await.unwrap();
        assert_eq!(preset.fresh_secs, want, "{raw:?}");
    }
    let raw = sqlx::query(
        "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id, \
         fresh_secs) VALUES ($1, 'raw', 'p', $2, '/w', 'claude-code', 30)",
    )
    .bind(a.user)
    .bind(a.machine)
    .execute(&pool)
    .await;
    assert!(raw.is_err(), "the table refuses an unclamped fresh_secs");
}

async fn insert_external(pool: &PgPool, owner: &Owner, mode: &str) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO presets (user_id, name, prompt, machine_id, working_dir, adapter_id, \
         permission_mode, external_sources) VALUES ($1, $3, 'p', $2, '/w', 'claude-code', \
         NULLIF($3, 'none'), true)",
    )
    .bind(owner.user)
    .bind(owner.machine)
    .bind(mode)
    .execute(pool)
    .await
    .map(|_| ())
}

#[tokio::test]
async fn an_external_machine_preset_must_be_guarded_in_the_table() {
    let Some(pool) = pool("an_external_machine_preset_must_be_guarded_in_the_table").await else {
        return;
    };
    let a = seed_owner(&pool, "ga").await;
    for mode in ["none", "yolo", "whip"] {
        assert!(insert_external(&pool, &a, mode).await.is_err(), "{mode}");
    }
    for mode in ["ask", "auto"] {
        assert!(insert_external(&pool, &a, mode).await.is_ok(), "{mode}");
    }
    let req = LaunchPresetRequest {
        external_sources: true,
        permission_mode: Some(PermissionMode::Ask),
        ..machine_request(&a, "guarded")
    };
    let preset = create(&pool, a.user, &validate(&req).unwrap()).await.unwrap();
    assert_eq!(preset.permission_mode, Some(PermissionMode::Ask));
    assert!(preset.external_sources);
}

#[tokio::test]
async fn deleting_a_preset_cascades_its_lanes() {
    let Some(pool) = pool("deleting_a_preset_cascades_its_lanes").await else { return };
    let a = seed_owner(&pool, "ca").await;
    let preset = create(&pool, a.user, &machine(&a, "laned")).await.unwrap();
    let lane: Uuid = sqlx::query_scalar(
        "INSERT INTO inbox_lanes (user_id, key, preset_id) VALUES ($1, $2, $3) RETURNING id",
    )
    .bind(a.user)
    .bind(format!("preset:{}:ops", preset.id))
    .bind(preset.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO inbox_events (lane_id, user_id, source, body) VALUES ($1, $2, 'timer', 'x')",
    )
    .bind(lane)
    .bind(a.user)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(lane_count(&pool, preset.id).await, 1);

    assert!(delete(&pool, Some(a.user), preset.id).await.unwrap());
    assert_eq!(lane_count(&pool, preset.id).await, 0);
    let events: i64 = sqlx::query_scalar("SELECT count(*) FROM inbox_events WHERE lane_id = $1")
        .bind(lane)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(events, 0);

    let dangling =
        sqlx::query("INSERT INTO inbox_lanes (user_id, key, preset_id) VALUES ($1, 'k', $2)")
            .bind(a.user)
            .bind(Uuid::new_v4())
            .execute(&pool)
            .await;
    assert!(dangling.is_err(), "a lane cannot name a missing preset");
}

#[tokio::test]
async fn preset_references_must_belong_to_the_owner() {
    let Some(pool) = pool("preset_references_must_belong_to_the_owner").await else { return };
    let (a, b) = (seed_owner(&pool, "ra").await, seed_owner(&pool, "rb").await);
    let foreign_machine =
        LaunchPresetRequest { machine_id: Some(b.machine), ..machine_request(&a, "theirs") };
    let refused = create(&pool, a.user, &validate(&foreign_machine).unwrap()).await;
    assert!(matches!(refused, Err(WriteError::Invalid(_))), "{refused:?}");

    let foreign_dispatcher = LaunchPresetRequest {
        name: "container".into(),
        prompt: "p".into(),
        dispatcher_id: Some(b.dispatcher),
        profile: Some("default".into()),
        ..LaunchPresetRequest::default()
    };
    let refused = create(&pool, a.user, &validate(&foreign_dispatcher).unwrap()).await;
    assert!(matches!(refused, Err(WriteError::Invalid(_))), "{refused:?}");
    let own_dispatcher =
        LaunchPresetRequest { dispatcher_id: Some(a.dispatcher), ..foreign_dispatcher };
    let container = create(&pool, a.user, &validate(&own_dispatcher).unwrap()).await.unwrap();
    assert_eq!(container.profile.as_deref(), Some("default"));

    let theirs = create(&pool, b.user, &machine(&b, "b-next")).await.unwrap();
    let chain_foreign =
        LaunchPresetRequest { then_preset_id: Some(theirs.id), ..machine_request(&a, "chain") };
    let refused = create(&pool, a.user, &validate(&chain_foreign).unwrap()).await;
    assert!(matches!(refused, Err(WriteError::Invalid(_))), "{refused:?}");

    let first = create(&pool, a.user, &machine(&a, "first")).await.unwrap();
    let chain = machine_request(&a, "first");
    let chain = LaunchPresetRequest { then_preset_id: Some(container.id), ..chain };
    let chained = update(&pool, Some(a.user), first.id, &validate(&chain).unwrap()).await;
    assert_eq!(chained.unwrap().unwrap().then_preset_id, Some(container.id));
    let to_self = machine_request(&a, "first");
    let to_self = LaunchPresetRequest { then_preset_id: Some(first.id), ..to_self };
    let refused = update(&pool, Some(a.user), first.id, &validate(&to_self).unwrap()).await;
    assert!(matches!(refused, Err(WriteError::Invalid(_))), "{refused:?}");

    assert!(delete(&pool, Some(a.user), container.id).await.unwrap());
    let unchained = get(&pool, None, first.id).await.unwrap().unwrap();
    assert_eq!(unchained.then_preset_id, None, "deleting the next preset unlinks the chain");
}

#[tokio::test]
async fn changing_a_preset_cron_clears_its_next_fire() {
    let Some(pool) = pool("changing_a_preset_cron_clears_its_next_fire").await else { return };
    let a = seed_owner(&pool, "cr").await;
    let daily = machine_request(&a, "daily");
    let daily = LaunchPresetRequest { cron: Some("0 9 * * *".into()), ..daily };
    let preset = create(&pool, a.user, &validate(&daily).unwrap()).await.unwrap();
    sqlx::query("UPDATE presets SET next_fire_at = now() + interval '1 hour' WHERE id = $1")
        .bind(preset.id)
        .execute(&pool)
        .await
        .unwrap();
    let same = update(&pool, None, preset.id, &validate(&daily).unwrap()).await.unwrap().unwrap();
    assert!(same.next_fire_at.is_some(), "an unchanged cron keeps its next fire");
    let moved = LaunchPresetRequest { cron: Some("0 10 * * *".into()), ..daily };
    let moved = update(&pool, None, preset.id, &validate(&moved).unwrap()).await.unwrap().unwrap();
    assert_eq!(moved.next_fire_at, None);
    assert_eq!(moved.cron.as_deref(), Some("0 10 * * *"));
}
