use axum::response::IntoResponse;
use uuid::Uuid;

use super::*;
use cctui_server_kernel::auth::Scope;

fn caller(user_id: Uuid) -> AuthContext {
    AuthContext {
        user_id,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(Scope::Read).collect(),
    }
}

async fn test_pool(test_name: &str) -> Option<sqlx::PgPool> {
    let url = cctui_server_kernel::test_support::db_url(test_name)?;
    Some(
        sqlx::postgres::PgPoolOptions::new()
            .max_connections(2)
            .connect(&url)
            .await
            .expect("connect test db"),
    )
}

fn admin(user_id: Uuid) -> AuthContext {
    AuthContext { scopes: [Scope::Read, Scope::Admin].into_iter().collect(), ..caller(user_id) }
}

/// A machine and an active session on it, both owned by `uid`.
async fn owned_session(pool: &sqlx::PgPool, uid: Uuid) -> (Uuid, String) {
    let machine = Uuid::new_v4();
    let sid = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, 'm', $3)")
        .bind(machine)
        .bind(uid)
        .bind(format!("mklab-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, machine_uuid, user_id, working_dir, status) \
         VALUES ($1, $2, $2, $3, '/w', 'active')",
    )
    .bind(&sid)
    .bind(machine)
    .bind(uid)
    .execute(pool)
    .await
    .unwrap();
    (machine, sid)
}

async fn owner_of(pool: &sqlx::PgPool, label_id: &str) -> Uuid {
    sqlx::query_scalar("SELECT user_id FROM labels WHERE id = $1")
        .bind(Uuid::parse_str(label_id).unwrap())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn user(pool: &sqlx::PgPool) -> Uuid {
    let uid = Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("label-{uid}"))
        .bind(format!("hlabel-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    uid
}

/// Labels are per-user: a second tenant neither sees, renames nor deletes
/// them, and the same name on both sides stays two distinct labels.
#[tokio::test]
async fn labels_are_not_visible_across_users() {
    let Some(pool) = test_pool("labels_are_not_visible_across_users").await else {
        return;
    };
    let (mine, theirs) = (user(&pool).await, user(&pool).await);
    let state = AppState::for_test(pool.clone());
    let name = format!("lane-{}", Uuid::new_v4());

    let (_, Json(label)) = create_label(
        State(state.clone()),
        Extension(caller(mine)),
        Json(CreateLabelRequest {
            name: name.clone(),
            color: "#abcdef".into(),
            session_id: None,
            machine_id: None,
        }),
    )
    .await
    .unwrap();

    let Json(listed) = list_labels(State(state.clone()), Extension(caller(theirs))).await.unwrap();
    assert!(
        !listed.labels.iter().any(|l| l.id == label.id),
        "another user's label must not be listed"
    );

    let renamed = update_label(
        State(state.clone()),
        Extension(caller(theirs)),
        Path(label.id.clone()),
        Json(UpdateLabelRequest { name: Some("stolen".into()), color: None }),
    )
    .await;
    assert!(renamed.is_err(), "another user's label must not be renameable");

    delete_label(State(state.clone()), Extension(caller(theirs)), Path(label.id.clone()))
        .await
        .unwrap();
    let still_there: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM labels WHERE id = $1)")
            .bind(Uuid::parse_str(&label.id).unwrap())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(still_there, "another user's delete must not remove the label");

    // The same name on another user is a new label, not an upsert of mine.
    let (_, Json(copy)) = create_label(
        State(state.clone()),
        Extension(caller(theirs)),
        Json(CreateLabelRequest {
            name,
            color: "#123456".into(),
            session_id: None,
            machine_id: None,
        }),
    )
    .await
    .unwrap();
    assert_ne!(copy.id, label.id);

    let Json(mine_listed) = list_labels(State(state), Extension(caller(mine))).await.unwrap();
    assert!(mine_listed.labels.iter().any(|l| l.id == label.id && l.color == "#abcdef"));
}

/// An admin's label lands where the session's labels live: the owner of
/// the session or machine it names, else the admin itself. A non-admin's
/// hint is ignored, its labels are always its own.
#[tokio::test]
async fn admin_label_joins_the_named_owner() {
    let Some(pool) = test_pool("admin_label_joins_the_named_owner").await else {
        return;
    };
    let (root, owner, other) = (user(&pool).await, user(&pool).await, user(&pool).await);
    let (machine, sid) = owned_session(&pool, owner).await;
    let state = AppState::for_test(pool.clone());
    let create = |ctx: AuthContext, session_id: Option<String>, machine_id: Option<String>| {
        let state = state.clone();
        async move {
            let (_, Json(label)) = create_label(
                State(state),
                Extension(ctx),
                Json(CreateLabelRequest {
                    name: format!("hint-{}", Uuid::new_v4()),
                    color: "#abcdef".into(),
                    session_id,
                    machine_id,
                }),
            )
            .await
            .unwrap();
            label.id
        }
    };

    let by_session = create(admin(root), Some(sid.clone()), None).await;
    assert_eq!(owner_of(&pool, &by_session).await, owner);
    let by_machine = create(admin(root), None, Some(machine.to_string())).await;
    assert_eq!(owner_of(&pool, &by_machine).await, owner);
    let unhinted = create(admin(root), None, None).await;
    assert_eq!(owner_of(&pool, &unhinted).await, root);
    let ignored = create(caller(other), Some(sid), Some(machine.to_string())).await;
    assert_eq!(owner_of(&pool, &ignored).await, other);
}

/// Attaching another user's label (an admin's pick) puts the session
/// owner's label of the same name on the session, never the foreign row;
/// a non-admin still cannot attach a label it does not see.
#[tokio::test]
async fn attach_maps_a_foreign_label_onto_the_owner() {
    let Some(pool) = test_pool("attach_maps_a_foreign_label_onto_the_owner").await else {
        return;
    };
    let (root, owner) = (user(&pool).await, user(&pool).await);
    let (_, sid) = owned_session(&pool, owner).await;
    let state = AppState::for_test(pool.clone());
    let name = format!("Pick-{}", Uuid::new_v4());
    let (_, Json(foreign)) = create_label(
        State(state.clone()),
        Extension(admin(root)),
        Json(CreateLabelRequest {
            name: name.clone(),
            color: "120".into(),
            session_id: None,
            machine_id: None,
        }),
    )
    .await
    .unwrap();
    assert_eq!(owner_of(&pool, &foreign.id).await, root);

    let denied = attach_label(
        State(state.clone()),
        Extension(caller(owner)),
        Path(sid.clone()),
        Json(AttachLabelRequest { label_id: foreign.id.clone() }),
    )
    .await;
    assert!(denied.is_err(), "a non-admin cannot attach a label it does not see");

    for _ in 0..2 {
        attach_label(
            State(state.clone()),
            Extension(admin(root)),
            Path(sid.clone()),
            Json(AttachLabelRequest { label_id: foreign.id.clone() }),
        )
        .await
        .unwrap();
    }
    let on_session: Vec<(Uuid, String, String, Uuid)> = sqlx::query_as(
        "SELECT l.id, l.name, l.color, l.user_id FROM session_labels sl \
           JOIN labels l ON l.id = sl.label_id WHERE sl.session_id = $1",
    )
    .bind(&sid)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(on_session.len(), 1, "attached once, as the owner's label");
    let (id, got_name, color, user_id) = &on_session[0];
    assert_eq!((got_name, color.as_str(), *user_id), (&name, "120", owner));
    assert_ne!(id.to_string(), foreign.id);
}

#[tokio::test]
async fn db_failure_is_opaque_500() {
    let pool = sqlx::PgPool::connect_lazy("postgres://invalid").unwrap();
    pool.close().await;
    let resp = list_labels(State(AppState::for_test(pool)), Extension(caller(Uuid::new_v4())))
        .await
        .unwrap_err()
        .into_response();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, format!(r#"{{"error":"{}"}}"#, cctui_server_kernel::error::DB_ERROR));
}
