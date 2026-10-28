//! Administrative identity flows on execution storage without workspace services.
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::routing::{delete, get, post};
use axum::{Extension, Router};
use cctui_server_core::auth::{AuthConfig, auth_middleware};
use cctui_server_core::identity::{admin, me};
use cctui_server_core::state::CoreState;
use serde_json::{Value, json};
use tower::ServiceExt;

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    token: &str,
    body: Value,
) -> (StatusCode, Value) {
    let response = app
        .clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

async fn execution_state() -> Option<CoreState> {
    let url = ["DATABASE_URL", "TEST_DATABASE_URL"]
        .iter()
        .filter_map(|key| std::env::var(key).ok())
        .find(|value| !value.trim().is_empty());
    let Some(url) = url else {
        assert!(std::env::var_os("CI").is_none(), "identity contract needs a test database in CI");
        eprintln!("skipping identity contract: no test database");
        return None;
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .after_connect(|conn, _| {
            Box::pin(async move {
                sqlx::query("SET search_path = pg_temp").execute(conn).await?;
                Ok(())
            })
        })
        .connect(&url)
        .await
        .unwrap();
    for ddl in [
        "CREATE TEMP TABLE users (LIKE public.users INCLUDING ALL)",
        "CREATE TEMP TABLE auth_keys (LIKE public.auth_keys INCLUDING ALL)",
        "CREATE TEMP TABLE user_tokens (LIKE public.user_tokens INCLUDING ALL)",
        "CREATE TEMP TABLE user_acls (LIKE public.user_acls INCLUDING ALL)",
        "CREATE TEMP TABLE key_acls (LIKE public.key_acls INCLUDING ALL)",
        "CREATE TEMP TABLE machines (LIKE public.machines INCLUDING ALL)",
        "CREATE TEMP TABLE events (LIKE public.events INCLUDING ALL)",
    ] {
        sqlx::query(ddl).execute(&pool).await.unwrap();
    }
    let auth = AuthConfig::new(vec!["fixture-admin".into()], pool.clone());
    auth.seed_admin().await;
    Some(CoreState::new(
        pool,
        auth.clone(),
        cctui_server_core::bus::Bus::new(Box::new(cctui_server_core::bus::NoopTransport)),
        cctui_server_core::client::build_http_client(),
        std::sync::Arc::new(cctui_server_core::dispatchers::Registry::new()),
    ))
}

#[tokio::test]
async fn headless_admin_provisions_scoped_identity_and_revokes_cached_credentials() {
    let Some(state) = execution_state().await else { return };
    let auth = state.auth_config.clone();
    let mut notices = state.bus.subscribe_server();
    let app = Router::new()
        .route("/admin/users", post(admin::create_user))
        .route("/users/{id}/keys", post(admin::mint_user_key))
        .route("/users/{id}/keys/{kid}", delete(admin::revoke_user_key))
        .route("/enroll", post(cctui_server_core::targets::enrollment::enroll))
        .route("/deenroll", post(cctui_server_core::targets::enrollment::deenroll))
        .route("/me", get(me::me))
        .route("/me/key", delete(me::revoke_current_key))
        .with_state(state)
        .layer(axum::middleware::from_fn(auth_middleware))
        .layer(Extension(auth));
    let (status, user) =
        request(&app, "POST", "/admin/users", "fixture-admin", json!({"name":"automation"})).await;
    assert_eq!(status, StatusCode::OK);
    let primary = user["key"].as_str().unwrap();
    let uid = user["id"].as_str().unwrap();
    let (status, _) =
        request(&app, "POST", "/admin/users", primary, json!({"name":"denied"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, key) = request(
        &app,
        "POST",
        &format!("/users/{uid}/keys"),
        primary,
        json!({"label":"read-only", "scopes":["read", "admin"]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(key["scopes"], json!(["read"]), "a key cannot raise its owner's ceiling");
    let token = key["key"].as_str().unwrap();
    let (status, identity) = request(&app, "GET", "/me", token, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["user_id"], user["id"]);
    assert_eq!(identity["scopes"], json!(["read"]));
    assert!(
        !identity.to_string().contains(token),
        "identity responses expose only a token preview"
    );
    let (status, _) = request(&app, "POST", "/enroll", token, json!({"hostname":"denied"})).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, machine) = request(
        &app,
        "POST",
        "/enroll",
        primary,
        json!({"hostname":"executor", "kind":"persistent"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let machine_key = machine["machine_key"].as_str().unwrap();
    let audit = tokio::time::timeout(std::time::Duration::from_secs(3), notices.recv())
        .await
        .unwrap()
        .unwrap();
    let cctui_proto::ws::ServerEvent::Event { event } = audit.event.as_ref() else {
        panic!("expected enrollment audit")
    };
    assert_eq!(event.kind, "machine.enrolled");
    let (status, identity) = request(&app, "GET", "/me", machine_key, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(identity["machine_id"], machine["machine_id"]);
    let (status, _) = request(&app, "POST", "/deenroll", machine_key, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(&app, "GET", "/me", machine_key, Value::Null).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "deenrollment purges both unified and legacy machine credentials"
    );
    let (status, _) = request(
        &app,
        "DELETE",
        &format!("/users/{uid}/keys/{}", key["id"].as_str().unwrap()),
        primary,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(&app, "GET", "/me", token, Value::Null).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "revocation purges an already-cached credential");
    let (status, _) = request(&app, "DELETE", "/me/key", primary, Value::Null).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = request(&app, "GET", "/me", primary, Value::Null).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "self-revocation closes the legacy credential path too"
    );
}
