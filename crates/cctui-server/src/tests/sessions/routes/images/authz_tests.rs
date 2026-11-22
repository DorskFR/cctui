//! The GET route through the real route table and its per-route guard: a blob is
//! readable by whoever may read its owning session, from any transcript, and
//! only under that session's id.
use axum::Router;
use axum::body::Body;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use tower::ServiceExt;
use uuid::Uuid;

use crate::state::AppState;
use cctui_server_kernel::auth::{AuthContext, Scope};

const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];

fn app(state: &AppState, viewer: Uuid) -> Router {
    let ctx = AuthContext {
        user_id: viewer,
        key_id: Uuid::new_v4(),
        machine_id: None,
        scopes: std::iter::once(Scope::Read).collect(),
    };
    let cfg = state.auth_config.clone();
    let (api, _, _) = crate::build_api_routes().into_parts();
    api.layer(middleware::from_fn(move |mut req: Request, next: Next| {
        let ctx = ctx.clone();
        let cfg = cfg.clone();
        async move {
            req.extensions_mut().insert(ctx);
            req.extensions_mut().insert(cfg);
            next.run(req).await
        }
    }))
    .with_state(state.clone())
}

async fn get(state: &AppState, viewer: Uuid, session: &str, image: Uuid) -> StatusCode {
    let uri = format!("/sessions/{session}/images/{image}");
    let req = Request::builder().uri(uri).body(Body::empty()).unwrap();
    app(state, viewer).oneshot(req).await.unwrap().status()
}

async fn user_with_session(pool: &sqlx::PgPool) -> (Uuid, Uuid, String) {
    let (uid, machine) = (Uuid::new_v4(), Uuid::new_v4());
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, $2, $3)")
        .bind(uid)
        .bind(format!("img-{uid}"))
        .bind(format!("kh-{uid}"))
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO machines (id, user_id, name, key_hash) VALUES ($1, $2, $3, $4)")
        .bind(machine)
        .bind(uid)
        .bind(machine.to_string())
        .bind(format!("kh-{machine}"))
        .execute(pool)
        .await
        .unwrap();
    (uid, machine, session_on(pool, uid, machine).await)
}

async fn session_on(pool: &sqlx::PgPool, uid: Uuid, machine: Uuid) -> String {
    let sid = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO sessions (id, machine_id, working_dir, user_id, machine_uuid) \
         VALUES ($1, 'm', '/w', $2, $3)",
    )
    .bind(&sid)
    .bind(uid)
    .bind(machine)
    .execute(pool)
    .await
    .unwrap();
    sid
}

async fn image_in(pool: &sqlx::PgPool, session: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO session_images (session_id, sha256, media_type, byte_len, bytes) \
         VALUES ($1, $2, 'image/png', $3, $4) RETURNING id",
    )
    .bind(session)
    .bind(Uuid::new_v4().to_string())
    .bind(i32::try_from(PNG.len()).unwrap())
    .bind(PNG)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn an_image_is_readable_from_its_owning_session_by_whoever_may_read_it() {
    let Some(url) = cctui_server_kernel::test_support::db_url("image_cross_session_authz") else {
        return;
    };
    let pool = crate::db::connect(&url).await.expect("connect test db");
    let state = AppState::for_test(pool.clone());
    let (owner, machine, sender) = user_with_session(&pool).await;
    let recipient = session_on(&pool, owner, machine).await;
    let (stranger, _, strangers_own) = user_with_session(&pool).await;
    let image = image_in(&pool, &sender).await;

    assert_eq!(get(&state, owner, &sender, image).await, StatusCode::OK);
    assert_eq!(get(&state, stranger, &sender, image).await, StatusCode::FORBIDDEN);
    assert_eq!(
        get(&state, owner, &recipient, image).await,
        StatusCode::NOT_FOUND,
        "a readable session cannot vouch for another session's blob"
    );
    assert_eq!(get(&state, stranger, &strangers_own, image).await, StatusCode::NOT_FOUND);
}
