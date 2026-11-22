use super::{
    PluginEnableRequest, PluginInstanceSettingsRequest, catalog, get_settings, install,
    put_settings, rotate_proxy_secret, set_enabled, uninstall,
};
use crate::state::AppState;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use cctui_server_kernel::auth::AuthContext;
use uuid::Uuid;

fn user() -> AuthContext {
    AuthContext {
        user_id: Uuid::new_v4(),
        key_id: Uuid::nil(),
        machine_id: None,
        scopes: std::iter::once(cctui_server_kernel::auth::Scope::Read).collect(),
    }
}

fn state() -> AppState {
    AppState::for_test(sqlx::PgPool::connect_lazy("postgres://unused@localhost/none").unwrap())
}

#[tokio::test]
async fn non_admins_get_403_before_any_work() {
    let req = Request::builder().body(axum::body::Body::empty()).unwrap();
    let (status, _) = install(State(state()), Extension(user()), req).await.unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = set_enabled(
        State(state()),
        Extension(user()),
        Path("demo".into()),
        Json(PluginEnableRequest { enabled: true }),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) =
        uninstall(State(state()), Extension(user()), Path("demo".into())).await.unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = catalog(State(state()), Extension(user())).await.unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) =
        get_settings(State(state()), Extension(user()), Path("demo".into())).await.unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = put_settings(
        State(state()),
        Extension(user()),
        Path("demo".into()),
        Json(PluginInstanceSettingsRequest { values: std::collections::BTreeMap::new() }),
    )
    .await
    .unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = rotate_proxy_secret(State(state()), Extension(user()), Path("demo".into()))
        .await
        .unwrap_err();
    assert_eq!(status, StatusCode::FORBIDDEN);
}
