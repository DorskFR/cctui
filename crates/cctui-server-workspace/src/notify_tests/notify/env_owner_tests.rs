use cctui_server_core::test_support::with_isolated_database;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use super::store::{NewNotification, Routing, emit};
use super::{instance_owner, kind};

const SERVER_NTFY: Routing = Routing { ntfy: true, env_owner: None };

pub async fn provisioned(pool: &PgPool) {
    cctui_server_core::schema::bootstrap(pool).await.unwrap();
    let mut conn = cctui_server_core::schema::migration_connection(pool).await.unwrap();
    crate::schema::provision(&mut conn, false).await.unwrap();
}

pub async fn user(pool: &PgPool, age_hours: i32) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO users (id, name, key_hash, created_at) \
         VALUES ($1, $2, $3, now() - make_interval(hours => $4))",
    )
    .bind(id)
    .bind(format!("env-owner-{id}"))
    .bind(format!("env-owner-key-{id}"))
    .bind(age_hours)
    .execute(pool)
    .await
    .unwrap();
    id
}

pub async fn endpoint(pool: &PgPool, who: Uuid, config: Value) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO notification_endpoints (user_id, channel, label, config) \
         VALUES ($1, 'ntfy', 'server', $2) RETURNING id",
    )
    .bind(who)
    .bind(config)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn env_endpoints(pool: &PgPool, who: Uuid) -> Vec<(Uuid, bool)> {
    sqlx::query_as(
        "SELECT id, enabled FROM notification_endpoints \
         WHERE user_id = $1 AND config ->> 'source' = 'env'",
    )
    .bind(who)
    .fetch_all(pool)
    .await
    .unwrap()
}

async fn pushed_to(pool: &PgPool, notification: Uuid) -> Vec<Option<Uuid>> {
    sqlx::query_scalar(
        "SELECT endpoint_id FROM notification_deliveries \
         WHERE notification_id = $1 AND channel = 'ntfy'",
    )
    .bind(notification)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn alert(who: Uuid) -> NewNotification {
    NewNotification::new(who, kind::SYSTEM_ALERT, "disk full".into())
}

#[tokio::test]
async fn only_the_instance_owner_reaches_the_server_ntfy_topic() {
    with_isolated_database("notify_env_owner_routing", |pool| async move {
        provisioned(&pool).await;
        let owner = user(&pool, 2).await;
        let other = user(&pool, 1).await;
        assert_eq!(instance_owner(&pool).await, Some(owner));

        let theirs = emit(&pool, SERVER_NTFY, alert(other)).await.unwrap();
        assert!(env_endpoints(&pool, other).await.is_empty(), "no server endpoint seeded");
        assert!(pushed_to(&pool, theirs.id).await.is_empty(), "nothing for the server topic");

        let mine = emit(&pool, SERVER_NTFY, alert(owner)).await.unwrap();
        let env = env_endpoints(&pool, owner).await;
        assert_eq!(env.len(), 1);
        assert_eq!(pushed_to(&pool, mine.id).await, [Some(env[0].0)]);
    })
    .await;
}

#[tokio::test]
async fn a_stale_server_endpoint_of_a_non_owner_is_never_routed() {
    with_isolated_database("notify_env_owner_stale", |pool| async move {
        provisioned(&pool).await;
        user(&pool, 2).await;
        let other = user(&pool, 1).await;
        endpoint(&pool, other, json!({ "source": "env" })).await;
        let own =
            endpoint(&pool, other, json!({ "url": "https://ntfy.example", "topic": "me" })).await;

        let theirs = emit(&pool, SERVER_NTFY, alert(other)).await.unwrap();
        assert_eq!(pushed_to(&pool, theirs.id).await, [Some(own)], "their own topic only");
    })
    .await;
}

#[tokio::test]
async fn the_migration_disables_non_owner_server_endpoints_only() {
    with_isolated_database("notify_env_owner_migration", |pool| async move {
        provisioned(&pool).await;
        let owner = user(&pool, 2).await;
        let other = user(&pool, 1).await;
        let owner_env = endpoint(&pool, owner, json!({ "source": "env" })).await;
        let other_env = endpoint(&pool, other, json!({ "source": "env" })).await;
        let other_own =
            endpoint(&pool, other, json!({ "url": "https://ntfy.example", "topic": "me" })).await;

        let migration = crate::schema::MIGRATIONS.iter().find(|m| m.version == 15).unwrap();
        assert_eq!(migration.name, "env_endpoint_owner_only");
        sqlx::raw_sql(migration.sql).execute(&pool).await.unwrap();

        let enabled: Vec<(Uuid, bool)> = sqlx::query_as(
            "SELECT id, enabled FROM notification_endpoints WHERE id = ANY($1) ORDER BY id",
        )
        .bind(&[owner_env, other_env, other_own][..])
        .fetch_all(&pool)
        .await
        .unwrap();
        let mut expected = [(owner_env, true), (other_env, false), (other_own, true)];
        expected.sort_unstable();
        assert_eq!(enabled, expected, "disabled, not deleted");
    })
    .await;
}
