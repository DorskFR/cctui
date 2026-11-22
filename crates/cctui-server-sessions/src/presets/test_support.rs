use sqlx::PgPool;
use uuid::Uuid;

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
