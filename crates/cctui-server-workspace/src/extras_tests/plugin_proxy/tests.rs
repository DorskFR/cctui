use super::{PLUGIN_HEADER, SIG_HEADER, TS_HEADER, USER_ID_HEADER, USER_NAME_HEADER, sign};
use axum::http::header::HeaderMap;

/// This test's own plugin id: the `plugins` / `plugin_settings` tables are
/// shared by every test in the run.
const PLUGIN_ID: &str = "plugin-proxy-backend";

type RecordedCalls = std::sync::Arc<std::sync::Mutex<Vec<(String, String, HeaderMap)>>>;

/// An in-process upstream on an ephemeral port that records what it was sent.
async fn spawn_test_upstream() -> (String, RecordedCalls) {
    use axum::http::Request as HttpRequest;
    let seen: RecordedCalls = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let upstream = axum::Router::new().fallback(move |req: HttpRequest<axum::body::Body>| {
        let recorder = recorder.clone();
        async move {
            recorder.lock().unwrap().push((
                req.method().to_string(),
                req.uri().to_string(),
                req.headers().clone(),
            ));
            "upstream ok"
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, upstream).await.unwrap() });
    (base, seen)
}

/// Register a backend-declaring plugin in `state`, point it at `upstream` and
/// mint its proxy secret. The `TempDir` must outlive the plugin's files.
async fn install_backend_plugin(
    state: &crate::state::AppState,
    pool: &sqlx::PgPool,
    upstream: &str,
) -> (tempfile::TempDir, String) {
    let root = tempfile::tempdir().unwrap();
    crate::plugins::test_support::write_plugin(
        root.path(),
        PLUGIN_ID,
        r#","instanceSettings":[{"key":"upstream","label":"U","type":"url"}],"backend":{"upstreamSetting":"upstream"}"#,
    );
    let mut plugin = crate::plugins::load_plugin(&root.path().join(PLUGIN_ID)).unwrap();
    plugin.instance_enabled = true;
    let manifest = plugin.manifest.clone();
    state.plugins.upsert_installed(plugin);

    crate::plugin_settings::delete(pool, PLUGIN_ID).await.unwrap();
    crate::plugin_settings::write(
        pool,
        &manifest,
        &std::collections::BTreeMap::from([("upstream".to_owned(), upstream.to_owned())]),
    )
    .await
    .unwrap();
    let (secret, _) = crate::plugin_settings::ensure_proxy_secret(pool, PLUGIN_ID).await.unwrap();
    (root, secret)
}

fn enabled_flag(enabled: bool) -> serde_json::Value {
    serde_json::json!({ "plugins": { "enabled": { PLUGIN_ID: enabled } } })
}

async fn seed_user(pool: &sqlx::PgPool) -> uuid::Uuid {
    let user_id = uuid::Uuid::new_v4();
    sqlx::query("INSERT INTO users (id, name, key_hash) VALUES ($1, 'Proxy Tester', $2)")
        .bind(user_id)
        .bind(user_id.to_string())
        .execute(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO user_settings (user_id, version, data) VALUES ($1, 1, $2)")
        .bind(user_id)
        .bind(enabled_flag(true))
        .execute(pool)
        .await
        .unwrap();
    user_id
}

async fn set_enabled_for_user(pool: &sqlx::PgPool, user_id: uuid::Uuid, enabled: bool) {
    sqlx::query("UPDATE user_settings SET data = $2 WHERE user_id = $1")
        .bind(user_id)
        .bind(enabled_flag(enabled))
        .execute(pool)
        .await
        .unwrap();
}

fn read_ctx(user_id: uuid::Uuid) -> crate::auth::AuthContext {
    crate::auth::AuthContext {
        user_id,
        key_id: uuid::Uuid::nil(),
        machine_id: None,
        scopes: std::iter::once(crate::auth::Scope::Read).collect(),
    }
}

async fn call_backend(
    state: &crate::state::AppState,
    ctx: &crate::auth::AuthContext,
    sub_path: &str,
    request: axum::http::Request<axum::body::Body>,
) -> axum::response::Response {
    super::backend(
        axum::extract::State(state.clone()),
        axum::Extension(ctx.clone()),
        axum::extract::Path((PLUGIN_ID.to_owned(), sub_path.to_owned())),
        request,
    )
    .await
}

fn empty_request() -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder().body(axum::body::Body::empty()).unwrap()
}

#[tokio::test]
async fn a_test_upstream_sees_a_verifiable_identity_and_no_cctui_cookie() {
    let Some(url) = crate::routes::gateway::test_db_url("plugin_backend_proxy") else {
        return;
    };
    let pool = crate::db::connect(&url).await.expect("connect test db");
    let (base, seen) = spawn_test_upstream().await;
    let state = crate::state::AppState::for_test(pool.clone());
    let (_plugin_dir, secret) = install_backend_plugin(&state, &pool, &base).await;
    let user_id = seed_user(&pool).await;
    let ctx = read_ctx(user_id);

    let request = axum::http::Request::builder()
        .method("GET")
        .uri(format!("/api/v1/plugins/{PLUGIN_ID}/backend/v1/pulls?state=open"))
        .header(axum::http::header::COOKIE, "cctui_auth=secret; cctui_preview=t; theirs=1")
        .header(axum::http::header::AUTHORIZATION, "Bearer cctui-token")
        .header(USER_ID_HEADER, "spoofed")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = call_backend(&state, &ctx, "v1/pulls", request).await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let calls = seen.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    let (method, uri, headers) = &calls[0];
    assert_eq!(method, "GET");
    assert_eq!(uri, "/v1/pulls?state=open");
    assert!(headers.get(axum::http::header::AUTHORIZATION).is_none());
    assert_eq!(
        headers.get(axum::http::header::COOKIE).map(|v| v.to_str().unwrap()),
        Some("theirs=1"),
        "cctui's own cookies never reach a plugin backend"
    );
    assert_eq!(headers.get(PLUGIN_HEADER).unwrap(), PLUGIN_ID);
    assert_eq!(headers.get(USER_ID_HEADER).unwrap(), &user_id.to_string());
    assert_eq!(headers.get(USER_NAME_HEADER).unwrap(), "Proxy Tester");
    let ts: i64 = headers.get(TS_HEADER).unwrap().to_str().unwrap().parse().unwrap();
    assert_eq!(
        headers.get(SIG_HEADER).unwrap().to_str().unwrap(),
        sign(&secret, "GET", "/v1/pulls", ts, &user_id.to_string()),
        "the upstream can verify the signature with its own copy of the secret"
    );

    set_enabled_for_user(&pool, user_id, false).await;
    let resp = call_backend(&state, &ctx, "v1/pulls", empty_request()).await;
    assert_eq!(resp.status(), axum::http::StatusCode::FORBIDDEN);

    set_enabled_for_user(&pool, user_id, true).await;
    state.plugins.set_installed_enabled(PLUGIN_ID, false);
    let resp = call_backend(&state, &ctx, "v1/pulls", empty_request()).await;
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::NOT_FOUND,
        "an instance-disabled plugin is indistinguishable from an unknown one"
    );

    crate::plugin_settings::delete(&pool, PLUGIN_ID).await.unwrap();
    sqlx::query("DELETE FROM users WHERE id = $1").bind(user_id).execute(&pool).await.unwrap();
}
